//! The TIFF 6.0 LZW decoder: MSB-first codes of 9 to 12 bits, clear code 256, end code 257, and
//! the "early change" width switch.

use super::{check, read_byte};
use crate::error::{Error, corrupt, unsupported};
use std::io::{self, BufRead, Read};
use std::sync::atomic::AtomicBool;

/// The code table and the expansion buffer, kept by the caller from chunk to chunk. A chunk reads
/// only the entries it wrote itself, so they need no clearing between chunks.
pub(super) struct Tables {
    prefix: [u16; 4096],
    suffix: [u8; 4096],
    buf: [u8; 4097],
}

impl Tables {
    pub(super) fn new() -> Box<Self> {
        Box::new(Self {
            prefix: [0; 4096],
            suffix: [0; 4096],
            buf: [0; 4097],
        })
    }
}

pub(crate) struct Lzw<'a, R> {
    r: R,
    cancel: &'a AtomicBool,
    acc: u32,
    nbits: u32,
    width: u32,
    next: usize,
    /// -1 after a clear code.
    prev: i32,
    t: &'a mut Tables,
    /// `t.buf[pend.0..pend.1]` is what the last code decoded to and `read` has not returned.
    pend: (usize, usize),
    done: bool,
}

impl<'a, R: BufRead> Lzw<'a, R> {
    pub(super) fn new(mut r: R, cancel: &'a AtomicBool, t: &'a mut Tables) -> Result<Self, Error> {
        // Two bytes however the reader chunks its reads (Go's `Peek(2)` blocks for them); they
        // go into the bit accumulator, so none is lost.
        let (mut acc, mut nbits, mut head) = (0u32, 0u32, [0u8; 2]);
        while nbits < 16 {
            let Ok(Some(b)) = read_byte(&mut r) else {
                break;
            };
            head[(nbits / 8) as usize] = b;
            acc = (acc << 8) | u32::from(b);
            nbits += 8;
        }
        if nbits == 16 && head[0] == 0 && head[1] & 1 != 0 {
            return Err(unsupported("old-style lzw"));
        }
        Ok(Self {
            r,
            cancel,
            acc,
            nbits,
            width: 9,
            next: 258,
            prev: -1,
            t,
            pend: (0, 0),
            done: false,
        })
    }

    fn code(&mut self) -> io::Result<usize> {
        while self.nbits < self.width {
            let Some(b) = read_byte(&mut self.r)? else {
                return Err(io::ErrorKind::UnexpectedEof.into());
            };
            self.acc = (self.acc << 8) | u32::from(b);
            self.nbits += 8;
        }
        self.nbits -= self.width;
        Ok(((self.acc >> self.nbits) & ((1 << self.width) - 1)) as usize)
    }

    /// Decodes one code into `pend`.
    fn step(&mut self) -> io::Result<()> {
        let c = self.code()?;
        match c {
            256 => {
                // A clear code makes no output: a long run of them must be cancellable.
                check(self.cancel).map_err(io::Error::other)?;
                self.width = 9;
                self.next = 258;
                self.prev = -1;
                return Ok(());
            }
            257 => {
                self.done = true;
                return Ok(());
            }
            _ => {}
        }
        let mut end = self.t.buf.len();
        let start;
        if c < self.next {
            start = self.expand(c, end);
        } else if c == self.next && self.prev >= 0 {
            // KwKwK: the previous string plus its own first byte.
            end -= 1;
            start = self.expand(self.prev as usize, end);
            self.t.buf[end] = self.t.buf[start];
            end += 1;
        } else {
            return Err(corrupt("lzw code").into());
        }
        if self.prev >= 0 && self.next < 4096 {
            self.t.prefix[self.next] = self.prev as u16;
            self.t.suffix[self.next] = self.t.buf[start];
            self.next += 1;
            if self.next >= (1 << self.width) - 1 && self.width < 12 {
                self.width += 1;
            }
        }
        self.prev = c as i32;
        self.pend = (start, end);
        Ok(())
    }

    /// Writes the string for code `c` backwards, ending at `end`, and returns its start.
    fn expand(&mut self, mut c: usize, end: usize) -> usize {
        let mut i = end;
        while c >= 258 {
            i -= 1;
            self.t.buf[i] = self.t.suffix[c];
            c = usize::from(self.t.prefix[c]);
        }
        i -= 1;
        self.t.buf[i] = c as u8;
        i
    }
}

impl<R: BufRead> Read for Lzw<'_, R> {
    fn read(&mut self, p: &mut [u8]) -> io::Result<usize> {
        if p.is_empty() {
            return Ok(0);
        }
        while self.pend.0 == self.pend.1 {
            if self.done {
                return Ok(0);
            }
            self.step()?;
        }
        let n = p.len().min(self.pend.1 - self.pend.0);
        p[..n].copy_from_slice(&self.t.buf[self.pend.0..self.pend.0 + n]);
        self.pend.0 += n;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    // Whether the strip is refused must not depend on how many bytes a read returns.
    #[test]
    fn old_style_refused_with_one_byte_reads() {
        let cancel = AtomicBool::new(false);
        let old = [0u8, 1, 0x80, 0x40];
        for cap in [1, 2, 4096] {
            let r = BufReader::with_capacity(cap, &old[..]);
            let mut t = Tables::new();
            assert!(
                matches!(
                    Lzw::new(r, &cancel, &mut t).err(),
                    Some(Error::Unsupported(_))
                ),
                "cap {cap}"
            );
        }
        // New style: the two bytes are still decoded (clear, 'A', end).
        let new = [0x80u8, 0x10, 0x60, 0x20];
        let mut out = Vec::new();
        let mut t = Tables::new();
        Lzw::new(BufReader::with_capacity(1, &new[..]), &cancel, &mut t)
            .ok()
            .unwrap()
            .read_to_end(&mut out)
            .unwrap();
        assert_eq!(out, b"A");
    }
}
