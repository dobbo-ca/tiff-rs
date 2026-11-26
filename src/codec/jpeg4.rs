//! A four-component baseline JPEG strip or tile, which jpeg-rs does not decode (it stops at
//! three components). libtiff hands such a chunk to libjpeg with no colour space (JCS_UNKNOWN):
//! the four samples come back as stored, an Adobe marker's transform ignored, and it refuses any
//! sampling but 1x1. This reads a single interleaved baseline scan the same way: Huffman,
//! dequantise, libjpeg's accurate integer IDCT (jidctint.c), 8 rows of MCUs at a time. Ported
//! from tiff-go's `internal/codec/jpeg4.go`.

use super::{Env, RowReader, check};
use crate::error::{Error, corrupt, limit, unsupported};
use crate::ledger::{Buf, Held};
use crate::readat::{ReadAt, SectionReader};
use std::sync::atomic::AtomicBool;

/// A canonical Huffman table (ITU T.81 F.2.2.3).
struct Huff {
    maxcode: [i32; 18],
    valptr: [usize; 17],
    mincode: [i32; 17],
    vals: Vec<u8>,
}

const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

#[derive(Clone, Copy, Default)]
struct Comp {
    q: u8,
    td: u8,
    ta: u8,
}

/// The entropy-coded data and its bit buffer.
struct Bits {
    /// The entropy-coded data after SOS.
    b: Vec<u8>,
    pos: usize,
    /// Bit buffer, `n` valid bits at the top.
    acc: u32,
    n: u32,
}

impl Bits {
    /// The next `n` (1..16) bits.
    fn bits(&mut self, n: u32) -> Result<i32, Error> {
        while self.n < n {
            let Some(&v) = self.b.get(self.pos) else {
                return Err(corrupt("truncated data"));
            };
            if v == 0xff {
                if self.b.get(self.pos + 1) != Some(&0) {
                    return Err(corrupt("jpeg marker in data"));
                }
                self.pos += 1;
            }
            self.pos += 1;
            self.acc |= u32::from(v) << (24 - self.n);
            self.n += 8;
        }
        let r = (self.acc >> (32 - n)) as i32;
        self.acc <<= n;
        self.n -= n;
        Ok(r)
    }

    fn sym(&mut self, h: Option<&Huff>) -> Result<u8, Error> {
        let Some(h) = h else {
            return Err(corrupt("jpeg table missing"));
        };
        let mut code = 0i32;
        for l in 1..=16 {
            code = code << 1 | self.bits(1)?;
            if h.maxcode[l] >= 0 && code <= h.maxcode[l] {
                let k = h.valptr[l] + (code - h.mincode[l]) as usize;
                if k >= h.vals.len() {
                    break;
                }
                return Ok(h.vals[k]);
            }
        }
        Err(corrupt("jpeg code"))
    }

    fn recv(&mut self, s: u32) -> Result<i32, Error> {
        if s == 0 {
            return Ok(0);
        }
        let mut v = self.bits(s)?;
        if v < 1 << (s - 1) {
            v = v.wrapping_sub((1 << s) - 1);
        }
        Ok(v)
    }
}

struct Jpeg4<'a> {
    cancel: &'a AtomicBool,
    bs: Bits,
    w: usize,
    h: usize,
    /// Natural order.
    qt: [[i32; 64]; 4],
    dc: [Option<Huff>; 4],
    ac: [Option<Huff>; 4],
    comp: [Comp; 4],
    pred: [i32; 4],
    /// MCUs between restarts, 0 for none.
    restart: usize,
    /// MCUs to the next restart.
    left: usize,
    /// MCU columns, rows.
    mx: usize,
    band: Buf<'a>,
    /// The next row (of the chunk) to hand out.
    next: usize,
    /// The charge of `bs.b`, until the reader drops.
    _held: Vec<Held<'a>>,
}

pub(super) fn new<'a, 'l: 'a, R: ReadAt>(
    src: SectionReader<'a, R>,
    tables: &[u8],
    w: usize,
    h: usize,
    band_bytes: u64,
    mut left: u64,
    cancel: &'a AtomicBool,
    env: &mut Env<'l>,
) -> Result<Box<dyn RowReader + 'a>, Error> {
    if w > 65500 || h > 65500 {
        return Err(unsupported(format!("jpeg frame {w}x{h}")));
    }
    let mx = w.div_ceil(8);
    let bb = mx as u64 * 8 * 8 * 4;
    if bb > band_bytes {
        return Err(limit("band bytes", bb, band_bytes));
    }
    let (mut b, mut held) = env.read_all(src, &mut left, cancel)?.into_parts();
    let band = env.alloc(bb, &mut left)?;
    let mut j = Jpeg4 {
        cancel,
        bs: Bits {
            b: Vec::new(),
            pos: 0,
            acc: 0,
            n: 0,
        },
        w,
        h,
        qt: [[0; 64]; 4],
        dc: Default::default(),
        ac: Default::default(),
        comp: [Comp::default(); 4],
        pred: [0; 4],
        restart: 0,
        left: 0,
        mx,
        band,
        next: 0,
        _held: Vec::new(),
    };
    // The tables stream, then the chunk: the data starts after the last stream's SOS.
    let mut start: Option<(usize, usize)> = None;
    for (which, s) in [tables, b.as_slice()].into_iter().enumerate() {
        if s.is_empty() {
            continue;
        }
        start = j.parse(s)?.map(|pos| (which, pos));
    }
    let Some((which, pos)) = start else {
        return Err(corrupt("jpeg header"));
    };
    j.bs.b = if which == 0 {
        let (mut v, tail) = env
            .alloc((tables.len() - pos) as u64, &mut left)?
            .into_parts();
        v.copy_from_slice(&tables[pos..]);
        held = tail;
        v
    } else {
        let len = b.len();
        b.copy_within(pos.., 0);
        b.truncate(len - pos);
        b
    };
    j._held.push(held);
    Ok(Box::new(j))
}

impl Jpeg4<'_> {
    /// Reads the segments of `s` up to SOS, returning where the entropy-coded data starts, or
    /// up to EOI (`None`).
    fn parse(&mut self, s: &[u8]) -> Result<Option<usize>, Error> {
        if s.len() < 2 || s[0] != 0xff || s[1] != 0xd8 {
            return Err(corrupt("jpeg header"));
        }
        let mut i = 2;
        loop {
            if i + 2 > s.len() || s[i] != 0xff {
                return Err(corrupt("jpeg segment"));
            }
            let m = s[i + 1];
            if m == 0xff {
                i += 1; // fill byte
                continue;
            }
            if m == 0xd9 {
                return Ok(None);
            }
            if i + 4 > s.len() {
                return Err(corrupt("jpeg segment"));
            }
            let l = usize::from(s[i + 2]) << 8 | usize::from(s[i + 3]);
            if l < 2 || i + 2 + l > s.len() {
                return Err(corrupt("jpeg segment"));
            }
            let seg = &s[i + 4..i + 2 + l];
            match m {
                0xdb => self.dqt(seg)?,
                0xc4 => self.dht(seg)?,
                0xc0 | 0xc1 => self.sof(seg)?,
                0xc2..=0xcf if m != 0xc4 && m != 0xc8 && m != 0xcc => {
                    return Err(unsupported("jpeg that is not baseline"));
                }
                0xdd => {
                    if seg.len() != 2 {
                        return Err(corrupt("jpeg restart interval"));
                    }
                    self.restart = usize::from(seg[0]) << 8 | usize::from(seg[1]);
                }
                0xda => {
                    self.sos(seg)?;
                    return Ok(Some(i + 2 + l));
                }
                _ => {}
            }
            i += 2 + l;
        }
    }

    fn dqt(&mut self, mut seg: &[u8]) -> Result<(), Error> {
        while !seg.is_empty() {
            let (pq, id) = (seg[0] >> 4, usize::from(seg[0] & 15));
            let n = 64 * (1 + usize::from(pq));
            if pq > 1 || id > 3 || seg.len() < 1 + n {
                return Err(corrupt("jpeg quantisation table"));
            }
            for k in 0..64 {
                let v = if pq == 1 {
                    i32::from(seg[1 + 2 * k]) << 8 | i32::from(seg[2 + 2 * k])
                } else {
                    i32::from(seg[1 + k])
                };
                self.qt[id][usize::from(ZIGZAG[k])] = v;
            }
            seg = &seg[1 + n..];
        }
        Ok(())
    }

    fn dht(&mut self, mut seg: &[u8]) -> Result<(), Error> {
        while !seg.is_empty() {
            let (tc, th) = (seg[0] >> 4, usize::from(seg[0] & 15));
            if tc > 1 || th > 3 || seg.len() < 17 {
                return Err(corrupt("jpeg huffman table"));
            }
            let mut counts = [0i32; 17];
            let mut n = 0usize;
            for k in 1..=16 {
                counts[k] = i32::from(seg[k]);
                n += usize::from(seg[k]);
            }
            if n > 256 || seg.len() < 17 + n {
                return Err(corrupt("jpeg huffman table"));
            }
            let mut h = Huff {
                maxcode: [0; 18],
                valptr: [0; 17],
                mincode: [0; 17],
                vals: seg[17..17 + n].to_vec(),
            };
            let (mut code, mut k) = (0i32, 0usize);
            for l in 1..=16 {
                h.valptr[l] = k;
                h.mincode[l] = code;
                code += counts[l];
                k += counts[l] as usize;
                h.maxcode[l] = code - 1;
                if counts[l] == 0 {
                    h.maxcode[l] = -1;
                }
                if code > 1 << l {
                    return Err(corrupt("jpeg huffman table"));
                }
                code <<= 1;
            }
            h.maxcode[17] = i32::MAX;
            if tc == 0 {
                self.dc[th] = Some(h);
            } else {
                self.ac[th] = Some(h);
            }
            seg = &seg[17 + n..];
        }
        Ok(())
    }

    fn sof(&mut self, seg: &[u8]) -> Result<(), Error> {
        if seg.len() != 6 + 4 * 3 || seg[0] != 8 || seg[5] != 4 {
            return Err(unsupported("jpeg that is not 8-bit with 4 components"));
        }
        if usize::from(seg[1]) << 8 | usize::from(seg[2]) != self.h
            || usize::from(seg[3]) << 8 | usize::from(seg[4]) != self.w
        {
            return Err(corrupt("jpeg frame does not match its chunk"));
        }
        for c in 0..4 {
            if seg[7 + 3 * c] != 0x11 || seg[8 + 3 * c] > 3 {
                return Err(unsupported("jpeg sampling")); // libtiff refuses it too
            }
            self.comp[c].q = seg[8 + 3 * c];
        }
        Ok(())
    }

    fn sos(&mut self, seg: &[u8]) -> Result<(), Error> {
        if seg.len() != 1 + 2 * 4 + 3
            || seg[0] != 4
            || seg[1 + 8] != 0
            || seg[2 + 8] != 63
            || seg[3 + 8] != 0
        {
            return Err(unsupported(
                "jpeg that is not one interleaved baseline scan",
            ));
        }
        for c in 0..4 {
            // components come in frame order
            let (td, ta) = (seg[2 + 2 * c] >> 4, seg[2 + 2 * c] & 15);
            if td > 3 || ta > 3 {
                return Err(corrupt("jpeg scan"));
            }
            self.comp[c].td = td;
            self.comp[c].ta = ta;
        }
        Ok(())
    }

    /// Decodes one 8x8 block of component `c` into its dequantised coefficients.
    fn block(&mut self, c: usize, out: &mut [i32; 64]) -> Result<(), Error> {
        *out = [0; 64];
        let cp = self.comp[c];
        let t = match self.bs.sym(self.dc[usize::from(cp.td)].as_ref()) {
            Ok(t) if t <= 11 => t,
            _ => return Err(corrupt("jpeg data")),
        };
        let d = self.bs.recv(u32::from(t))?;
        self.pred[c] = self.pred[c].wrapping_add(d);
        let q = &self.qt[usize::from(cp.q)];
        out[0] = self.pred[c].wrapping_mul(q[0]);
        let mut k = 1usize;
        while k < 64 {
            let rs = self.bs.sym(self.ac[usize::from(cp.ta)].as_ref())?;
            let (r, s) = (usize::from(rs >> 4), u32::from(rs & 15));
            if s == 0 {
                if r != 15 {
                    break;
                }
                k += 16;
                continue;
            }
            k += r;
            if k > 63 || s > 10 {
                return Err(corrupt("jpeg data"));
            }
            let v = self.bs.recv(s)?;
            let z = usize::from(ZIGZAG[k]);
            out[z] = v.wrapping_mul(q[z]);
            k += 1;
        }
        Ok(())
    }

    /// Byte-aligns and consumes an RSTn, resetting the predictors.
    fn restart_marker(&mut self) -> Result<(), Error> {
        let bs = &mut self.bs;
        bs.acc = 0;
        bs.n = 0;
        match bs.b.get(bs.pos..bs.pos + 2) {
            Some(&[0xff, m]) if (0xd0..=0xd7).contains(&m) => {}
            _ => return Err(corrupt("jpeg restart")),
        }
        bs.pos += 2;
        self.pred = [0; 4];
        Ok(())
    }

    fn decode_band(&mut self) -> Result<(), Error> {
        let mut blk = [0i32; 64];
        for x in 0..self.mx {
            if self.restart > 0 {
                // As Go: nothing counts MCU rows, so a band's first MCU never takes a restart
                // marker (jpeg4.go's mcuRow is always 0).
                if self.left == 0 && x > 0 {
                    self.restart_marker()?;
                }
                if self.left == 0 {
                    self.left = self.restart;
                }
                self.left -= 1;
            }
            for c in 0..4 {
                self.block(c, &mut blk)?;
                idct(&blk, &mut self.band[x * 32 + c..], self.mx * 32);
            }
        }
        Ok(())
    }
}

impl RowReader for Jpeg4<'_> {
    fn read_row(&mut self, dst: &mut [u8]) -> Result<(), Error> {
        if self.next >= self.h {
            return Err(corrupt("jpeg rows"));
        }
        if self.next.is_multiple_of(8) {
            check(self.cancel)?;
            self.decode_band()?;
        }
        let o = (self.next % 8) * self.mx * 32;
        dst.copy_from_slice(&self.band[o..o + 4 * self.w]);
        self.next += 1;
        Ok(())
    }
}

// jpeg_idct_islow of jidctint.c: out[(y*stride)+4*x] for the 8x8 block `inp`.
const CB: u32 = 13;
const PASS: u32 = 2;

fn idct(inp: &[i32; 64], out: &mut [u8], stride: usize) {
    let mut ws = [0i64; 64];
    let g = |i: usize| i64::from(inp[i]);
    for c in 0..8 {
        let (t10, t11, t12, t13, t0, t1, t2, t3) = idct1d(
            g(c),
            g(8 + c),
            g(16 + c),
            g(24 + c),
            g(32 + c),
            g(40 + c),
            g(48 + c),
            g(56 + c),
        );
        const S: u32 = CB - PASS;
        ws[c] = ds(t10 + t3, S);
        ws[56 + c] = ds(t10 - t3, S);
        ws[8 + c] = ds(t11 + t2, S);
        ws[48 + c] = ds(t11 - t2, S);
        ws[16 + c] = ds(t12 + t1, S);
        ws[40 + c] = ds(t12 - t1, S);
        ws[24 + c] = ds(t13 + t0, S);
        ws[32 + c] = ds(t13 - t0, S);
    }
    for r in 0..8 {
        let w = &ws[8 * r..];
        let (t10, t11, t12, t13, t0, t1, t2, t3) =
            idct1d(w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]);
        const S: u32 = CB + PASS + 3;
        let o = &mut out[r * stride..];
        o[0] = rl(ds(t10 + t3, S));
        o[28] = rl(ds(t10 - t3, S));
        o[4] = rl(ds(t11 + t2, S));
        o[24] = rl(ds(t11 - t2, S));
        o[8] = rl(ds(t12 + t1, S));
        o[20] = rl(ds(t12 - t1, S));
        o[12] = rl(ds(t13 + t0, S));
        o[16] = rl(ds(t13 - t0, S));
    }
}

fn ds(x: i64, n: u32) -> i64 {
    (x + (1 << (n - 1))) >> n
}

/// libjpeg's range_limit table indexed by x & 1023 (the level shift of 128 included).
fn rl(x: i64) -> u8 {
    let mut m = x & 1023;
    if m >= 512 {
        m -= 1024;
    }
    (m + 128).clamp(0, 255) as u8
}

/// One pass: the even part's tmp10..13 and the odd part's tmp0..3, scaled by `1 << CB`.
fn idct1d(
    i0: i64,
    i1: i64,
    i2: i64,
    i3: i64,
    i4: i64,
    i5: i64,
    i6: i64,
    i7: i64,
) -> (i64, i64, i64, i64, i64, i64, i64, i64) {
    let z1 = (i2 + i6) * 4433;
    let t2 = z1 + i6 * -15137;
    let t3 = z1 + i2 * 6270;
    let t0 = (i0 + i4) << CB;
    let t1 = (i0 - i4) << CB;
    let (t10, t13, t11, t12) = (t0 + t3, t0 - t3, t1 + t2, t1 - t2);
    let (mut t0, mut t1, mut t2, mut t3) = (i7, i5, i3, i1);
    let (mut z1, mut z2, mut z3, mut z4) = (t0 + t3, t1 + t2, t0 + t2, t1 + t3);
    let z5 = (z3 + z4) * 9633;
    t0 *= 2446;
    t1 *= 16819;
    t2 *= 25172;
    t3 *= 12299;
    z1 *= -7373;
    z2 *= -20995;
    z3 = z3 * -16069 + z5;
    z4 = z4 * -3196 + z5;
    t0 += z1 + z3;
    t1 += z2 + z4;
    t2 += z2 + z3;
    t3 += z1 + z4;
    (t10, t11, t12, t13, t0, t1, t2, t3)
}
