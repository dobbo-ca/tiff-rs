//! CCITT Group 3 (1-D) and Group 4 decoding, ported from golang.org/x/image/ccitt (reader.go);
//! see NOTICE. It decodes one row per call, into 1 bit per pixel with black as 1 (the reader
//! tiff-go builds with `Options{Invert: true}`).

use super::ccitt_tables::{BLACK_CODES, MODE_CODES, WHITE_CODES};
use super::{RowReader, alloc_in, check, read_err};
use crate::error::{Error, corrupt};
use crate::ledger::{Held, Ledger};
use std::io::BufRead;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;

/// The widest row this decoder takes, a guard against overflow.
const MAX_WIDTH: usize = 1 << 20;

/// A binary tree per code list: element 0 is invalid, the root is element 1. A positive entry
/// is a branch, a negative one is a leaf holding the complement of the value, 0 an invalid code.
struct Tables {
    mode: Vec<[i16; 2]>,
    white: Vec<[i16; 2]>,
    black: Vec<[i16; 2]>,
}

fn build(codes: &[(u16, &str)]) -> Vec<[i16; 2]> {
    let mut t = vec![[0i16; 2]; 2];
    for &(val, s) in codes {
        let mut node = 1usize;
        let n = s.len();
        for (i, c) in s.bytes().enumerate() {
            let b = usize::from(c == b'1');
            if i + 1 == n {
                t[node][b] = !(val as i16);
            } else {
                if t[node][b] <= 0 {
                    t.push([0, 0]);
                    t[node][b] = (t.len() - 1) as i16;
                }
                node = t[node][b] as usize;
            }
        }
    }
    t
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| Tables {
        mode: build(MODE_CODES),
        white: build(WHITE_CODES),
        black: build(BLACK_CODES),
    })
}

/// Why the next bit is not there.
enum Bit {
    Eof,
    Err(Error),
}

/// A failed EOL.
enum Eol {
    Missing,
    Err(Error),
}

struct Bits<R> {
    r: R,
    /// The high `n` bits hold the upcoming bits, MSB first.
    bits: u64,
    n: u32,
}

impl<R: BufRead> Bits<R> {
    fn next_bit(&mut self) -> Result<u64, Bit> {
        if self.n == 0 {
            let b = match self.r.fill_buf() {
                Ok(b) => match b.first() {
                    Some(&v) => v,
                    None => return Err(Bit::Eof),
                },
                Err(e) => return Err(Bit::Err(read_err(e))),
            };
            self.r.consume(1);
            self.bits = u64::from(b) << 56;
            self.n = 8;
        }
        let bit = self.bits >> 63;
        self.bits <<= 1;
        self.n -= 1;
        Ok(bit)
    }

    fn decode(&mut self, table: &[[i16; 2]]) -> Result<u32, Error> {
        let mut state = 1i32;
        loop {
            let bit = match self.next_bit() {
                Ok(b) => b,
                Err(Bit::Eof) => return Err(corrupt("ccitt: incomplete code")),
                Err(Bit::Err(e)) => return Err(e),
            };
            state = i32::from(table[state as usize][bit as usize]);
            if state < 0 {
                return Ok(!state as u32);
            } else if state == 0 {
                return Err(corrupt("ccitt: invalid code"));
            }
        }
    }

    /// Decodes the 12-bit EOL code 0000_0000_0001. A mismatch puts the bits back.
    fn decode_eol(&mut self) -> Result<(), Eol> {
        let (mut n_read, mut bits_read) = (0u32, 0u64);
        loop {
            let bit = match self.next_bit() {
                Ok(b) => b,
                Err(Bit::Eof) => return Err(Eol::Missing),
                Err(Bit::Err(e)) => return Err(Eol::Err(e)),
            };
            bits_read |= bit << (63 - n_read);
            n_read += 1;
            if n_read < 12 {
                if bit == 0 {
                    continue;
                }
            } else if bit != 0 {
                return Ok(());
            }
            self.bits = (self.bits >> n_read) | bits_read;
            self.n += n_read;
            return Err(Eol::Missing);
        }
    }
}

pub(crate) struct Ccitt<'a, R> {
    br: Bits<R>,
    group4: bool,
    width: usize,
    rows_remaining: usize,
    /// The current and previous rows, one byte per pixel: 0x00 black, 0xFF white. `prev` is
    /// valid from the second row. The vectors are the caller's, kept from chunk to chunk.
    curr: Vec<u8>,
    prev: Vec<u8>,
    /// Where `curr` and `prev` go back to on drop, for the next chunk. Owned while the chunk
    /// decodes, so the row loops index a `Vec` of `self` (no extra pointer to reload).
    home: &'a mut (Vec<u8>, Vec<u8>),
    have_prev: bool,
    /// The write index (the spec's a0).
    wi: usize,
    at_start_of_row: bool,
    pen_is_white: bool,
    started: bool,
    cancel: &'a AtomicBool,
    /// What the two row buffers may still be charged against, as a serial call sees it.
    alloc_left: u64,
    ledger: &'a Ledger,
    /// The two row buffers' charge, until the reader drops.
    _held: Option<(Held<'a>, Held<'a>)>,
}

impl<'a, R: BufRead> Ccitt<'a, R> {
    pub(super) fn new(
        r: R,
        group4: bool,
        width: usize,
        rows: usize,
        cancel: &'a AtomicBool,
        alloc_left: u64,
        ledger: &'a Ledger,
        home: &'a mut (Vec<u8>, Vec<u8>),
    ) -> Self {
        Self {
            br: Bits { r, bits: 0, n: 0 },
            group4,
            width,
            rows_remaining: rows,
            curr: std::mem::take(&mut home.0),
            prev: std::mem::take(&mut home.1),
            home,
            have_prev: false,
            wi: 0,
            at_start_of_row: true,
            pen_is_white: true,
            started: false,
            cancel,
            alloc_left,
            ledger,
            _held: None,
        }
    }

    fn pen_color(&self) -> u8 {
        if self.pen_is_white { 0xff } else { 0x00 }
    }

    fn fill(&mut self, from: usize, to: usize) {
        let c = self.pen_color();
        self.curr[from..to].fill(c);
    }

    fn decode_run(&mut self) -> Result<(), Error> {
        let t = tables();
        let table = if self.pen_is_white {
            &t.white
        } else {
            &t.black
        };
        let mut total = 0usize;
        loop {
            let n = self.br.decode(table)? as usize;
            total += n;
            if total > MAX_WIDTH {
                return Err(corrupt("ccitt: run length too long"));
            }
            // Anything 0x3F or below is a terminal code.
            if n <= 0x3f {
                break;
            }
        }
        if total > self.width - self.wi {
            return Err(corrupt("ccitt: run length overflows width"));
        }
        self.fill(self.wi, self.wi + total);
        self.wi += total;
        self.pen_is_white = !self.pen_is_white;
        Ok(())
    }

    /// b1 (or b2): the changing elements of the reference line to the right of a0.
    fn find_b(&self, b2: bool) -> usize {
        // The first row's reference line is all white: no changing element.
        if !self.have_prev {
            return self.width;
        }
        let prev = &self.prev;
        let mut i = self.wi;
        if self.at_start_of_row {
            // a0 is implicitly at -1, on a white pixel: b1 is the first black pixel of the
            // previous row, b2 the first white one after that.
            while i < prev.len() && prev[i] == 0xff {
                i += 1;
            }
            if b2 {
                while i < prev.len() && prev[i] == 0x00 {
                    i += 1;
                }
            }
            return i;
        }
        let pen = self.pen_color();
        let opposite = !pen;
        while i < prev.len() && prev[i] == opposite {
            i += 1;
        }
        while i < prev.len() && prev[i] == pen {
            i += 1;
        }
        if b2 {
            while i < prev.len() && prev[i] == opposite {
                i += 1;
            }
        }
        i
    }

    fn decode_row(&mut self, final_row: bool) -> Result<(), Error> {
        self.wi = 0;
        self.at_start_of_row = true;
        self.pen_is_white = true;
        if !self.group4 {
            while self.wi < self.width {
                // Zero-length runs make no progress: a row can take long.
                check(self.cancel)?;
                self.decode_run()?;
                self.at_start_of_row = false;
            }
            return match self.br.decode_eol() {
                Ok(()) => Ok(()),
                // The last row of a truncated stream may lack its EOL.
                Err(Eol::Missing) if final_row => Ok(()),
                Err(Eol::Missing) => Err(corrupt("ccitt: missing End-of-Line")),
                Err(Eol::Err(e)) => Err(e),
            };
        }
        while self.wi < self.width {
            check(self.cancel)?;
            let mode = self.br.decode(&tables().mode)?;
            match mode {
                0 => {
                    // Pass
                    let b2 = self.find_b(true);
                    if !(self.wi..=self.width).contains(&b2) {
                        return Err(corrupt("ccitt: invalid offset"));
                    }
                    self.fill(self.wi, b2);
                    self.wi = b2;
                }
                1 => {
                    // Horizontal: a1 then a2.
                    self.decode_run()?;
                    self.decode_run()?;
                }
                2..=8 => {
                    // Vertical: V0, VR1..3, VL1..3
                    let arg = [0isize, 1, 2, 3, -1, -2, -3][mode as usize - 2];
                    let a1 = self.find_b(false) as isize + arg;
                    if !(self.wi as isize..=self.width as isize).contains(&a1) {
                        return Err(corrupt("ccitt: invalid offset"));
                    }
                    self.fill(self.wi, a1 as usize);
                    self.wi = a1 as usize;
                    self.pen_is_white = !self.pen_is_white;
                }
                9 => return Err(corrupt("ccitt: unsupported mode")),
                _ => return Err(corrupt("ccitt: invalid mode")),
            }
            self.at_start_of_row = false;
        }
        Ok(())
    }
}

impl<R> Drop for Ccitt<'_, R> {
    fn drop(&mut self) {
        *self.home = (
            std::mem::take(&mut self.curr),
            std::mem::take(&mut self.prev),
        );
    }
}

impl<R: BufRead> RowReader for Ccitt<'_, R> {
    fn read_row(&mut self, dst: &mut [u8]) -> Result<(), Error> {
        check(self.cancel)?;
        if self.width > MAX_WIDTH {
            return Err(corrupt("ccitt: unsupported width"));
        }
        if !self.started {
            if !self.group4 {
                match self.br.decode_eol() {
                    Ok(()) => {}
                    Err(Eol::Missing) => return Err(corrupt("ccitt: missing End-of-Line")),
                    Err(Eol::Err(e)) => return Err(e),
                }
            }
            self.started = true;
            let n = self.width as u64;
            let h1 = alloc_in(&mut self.curr, n, &mut self.alloc_left, self.ledger)?;
            let h2 = alloc_in(&mut self.prev, n, &mut self.alloc_left, self.ledger)?;
            self._held = Some((h1, h2));
        }
        if self.rows_remaining == 0 {
            return Err(corrupt("truncated data"));
        }
        self.rows_remaining -= 1;
        self.decode_row(self.rows_remaining == 0)?;
        dst.fill(0);
        for (x, &px) in self.curr.iter().enumerate() {
            if px == 0 {
                dst[x >> 3] |= 0x80 >> (x & 7);
            }
        }
        std::mem::swap(&mut self.curr, &mut self.prev);
        self.have_prev = true;
        Ok(())
    }
}
