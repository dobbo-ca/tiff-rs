//! Synthetic TIFF files for tests: every layout and compression the decoder reads, from a
//! deterministic pixel pattern. Ported from tiff-go's `internal/fixture`, with Deflate written
//! as stored blocks (jpeg-rs and deflate-rs have no Deflate encoder here).
#![allow(dead_code)]

use std::ops::ControlFlow;
use std::sync::atomic::AtomicBool;
use tiff::{Error, Limits};

/// One IFD to write.
#[derive(Clone, Default)]
pub struct Page {
    pub w: usize,
    pub h: usize,
    pub spp: usize,
    pub bps: usize,
    pub photometric: usize,
    /// TIFF code; 1 when zero.
    pub compression: usize,
    pub predictor: usize,
    /// Strips (default `h`) unless `tile_w` is set.
    pub rows_per_strip: usize,
    pub tile_w: usize,
    pub tile_l: usize,
    pub color_map: Option<Vec<u16>>,
    /// Packed big-endian rows; `pattern` when `None`.
    pub pix: Option<Vec<u8>>,
    /// Pre-compressed strips or tiles, bypassing the encoder.
    pub chunks: Option<Vec<Vec<u8>>>,
    pub t4: u32,
    pub t6: u32,
    pub jpeg_tables: Option<Vec<u8>>,
}

/// A whole TIFF.
#[derive(Clone, Default)]
pub struct File {
    pub little_endian: bool,
    pub big: bool,
    pub pages: Vec<Page>,
}

/// A deterministic image: rows of packed samples, 16-bit big-endian.
pub fn pattern(w: usize, h: usize, spp: usize, bps: usize) -> Vec<u8> {
    let rb = (w * spp * bps).div_ceil(8);
    let mut b = vec![0u8; rb * h];
    for (i, v) in b.iter_mut().enumerate() {
        let (y, x) = (i / rb, i % rb);
        *v = (x * 7 + y * 13 + x * y) as u8;
        if bps < 8 && spp == 1 {
            // keep bilevel and low-depth data varied but legal
            *v = ((x * 5 + y * 3) * 31) as u8 ^ y as u8;
        }
    }
    b
}

struct Entry {
    tag: u16,
    typ: u16,
    vals: Vec<u64>,
}

struct Writer {
    b: Vec<u8>,
    le: bool,
    big: bool,
}

fn size(t: u16) -> usize {
    match t {
        1 | 7 => 1,
        3 => 2,
        4 => 4,
        16 => 8,
        _ => 0,
    }
}

impl Writer {
    fn u16(&mut self, v: u16) {
        let a = if self.le {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        };
        self.b.extend(a);
    }

    fn u32(&mut self, v: u32) {
        let a = if self.le {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        };
        self.b.extend(a);
    }

    fn u64(&mut self, v: u64) {
        let a = if self.le {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        };
        self.b.extend(a);
    }

    fn off(&mut self, v: u64) {
        if self.big {
            self.u64(v);
        } else {
            self.u32(v as u32);
        }
    }

    fn align(&mut self) {
        if self.b.len() % 2 == 1 {
            self.b.push(0);
        }
    }

    fn val(&mut self, t: u16, v: u64) {
        match t {
            1 | 7 => self.b.push(v as u8),
            3 => self.u16(v as u16),
            4 => self.u32(v as u32),
            16 => self.u64(v),
            _ => {}
        }
    }
}

impl File {
    /// Renders the file.
    pub fn bytes(&self) -> Vec<u8> {
        let mut w = Writer {
            b: Vec::new(),
            le: self.little_endian,
            big: self.big,
        };
        w.b.extend(if self.little_endian { b"II" } else { b"MM" });
        if self.big {
            w.u16(43);
            w.u16(8);
            w.u16(0);
        } else {
            w.u16(42);
        }
        let mut patch = w.b.len(); // where the next IFD offset goes
        w.off(0);
        for p in &self.pages {
            patch = w.page(p.clone(), patch);
        }
        w.b
    }
}

impl Writer {
    /// Writes chunks, value arrays and the IFD, pointing the field at `patch` to it; returns the
    /// position of the IFD's own next-offset field.
    fn page(&mut self, mut p: Page, patch: usize) -> usize {
        if p.compression == 0 {
            p.compression = 1;
        }
        if p.pix.is_none() {
            p.pix = Some(pattern(p.w, p.h, p.spp, p.bps));
        }
        let chunks = match p.chunks.take() {
            Some(c) => c,
            None => self.encode(&p),
        };
        let (mut offs, mut cnts) = (Vec::new(), Vec::new());
        for c in &chunks {
            self.align();
            offs.push(self.b.len() as u64);
            cnts.push(c.len() as u64);
            self.b.extend(c);
        }
        let sv = |v: &[usize]| v.iter().map(|&x| x as u64).collect::<Vec<_>>();
        let bps = vec![p.bps; p.spp];
        let mut es = vec![
            Entry {
                tag: 256,
                typ: 4,
                vals: sv(&[p.w]),
            },
            Entry {
                tag: 257,
                typ: 4,
                vals: sv(&[p.h]),
            },
            Entry {
                tag: 258,
                typ: 3,
                vals: sv(&bps),
            },
            Entry {
                tag: 259,
                typ: 3,
                vals: sv(&[p.compression]),
            },
            Entry {
                tag: 262,
                typ: 3,
                vals: sv(&[p.photometric]),
            },
            Entry {
                tag: 277,
                typ: 3,
                vals: sv(&[p.spp]),
            },
        ];
        if p.predictor != 0 {
            es.push(Entry {
                tag: 317,
                typ: 3,
                vals: sv(&[p.predictor]),
            });
        }
        if p.t4 != 0 {
            es.push(Entry {
                tag: 292,
                typ: 4,
                vals: vec![u64::from(p.t4)],
            });
        }
        if p.t6 != 0 {
            es.push(Entry {
                tag: 293,
                typ: 4,
                vals: vec![u64::from(p.t6)],
            });
        }
        if let Some(cm) = &p.color_map {
            es.push(Entry {
                tag: 320,
                typ: 3,
                vals: cm.iter().map(|&c| u64::from(c)).collect(),
            });
        }
        if let Some(t) = &p.jpeg_tables {
            es.push(Entry {
                tag: 347,
                typ: 7,
                vals: t.iter().map(|&c| u64::from(c)).collect(),
            });
        }
        let ot = if self.big { 16 } else { 4 };
        if p.tile_w > 0 {
            es.push(Entry {
                tag: 322,
                typ: 4,
                vals: sv(&[p.tile_w]),
            });
            es.push(Entry {
                tag: 323,
                typ: 4,
                vals: sv(&[p.tile_l]),
            });
            es.push(Entry {
                tag: 324,
                typ: ot,
                vals: offs,
            });
            es.push(Entry {
                tag: 325,
                typ: ot,
                vals: cnts,
            });
        } else {
            let rps = if p.rows_per_strip == 0 {
                p.h
            } else {
                p.rows_per_strip
            };
            es.push(Entry {
                tag: 273,
                typ: ot,
                vals: offs,
            });
            es.push(Entry {
                tag: 278,
                typ: 4,
                vals: sv(&[rps]),
            });
            es.push(Entry {
                tag: 279,
                typ: ot,
                vals: cnts,
            });
        }
        // out-of-line values first, then the directory; tags sorted.
        es.sort_by_key(|e| e.tag);
        let inl = if self.big { 8 } else { 4 };
        let mut where_ = vec![0u64; es.len()];
        for (i, en) in es.iter().enumerate() {
            if en.vals.len() * size(en.typ) > inl {
                self.align();
                where_[i] = self.b.len() as u64;
                for &v in &en.vals {
                    self.val(en.typ, v);
                }
            }
        }
        self.align();
        let at = self.b.len() as u64;
        if self.big {
            let a = if self.le {
                at.to_le_bytes()
            } else {
                at.to_be_bytes()
            };
            self.b[patch..patch + 8].copy_from_slice(&a);
        } else {
            let a = if self.le {
                (at as u32).to_le_bytes()
            } else {
                (at as u32).to_be_bytes()
            };
            self.b[patch..patch + 4].copy_from_slice(&a);
        }
        self.directory(&es, &where_, inl)
    }

    fn directory(&mut self, es: &[Entry], where_: &[u64], inl: usize) -> usize {
        if self.big {
            self.u64(es.len() as u64);
        } else {
            self.u16(es.len() as u16);
        }
        for (i, en) in es.iter().enumerate() {
            self.u16(en.tag);
            self.u16(en.typ);
            self.off(en.vals.len() as u64);
            if where_[i] != 0 {
                self.off(where_[i]);
                continue;
            }
            let start = self.b.len();
            for &v in &en.vals {
                self.val(en.typ, v);
            }
            while self.b.len() < start + inl {
                self.b.push(0);
            }
        }
        let at = self.b.len();
        self.off(0);
        at
    }

    /// Cuts `p.pix` into strips or tiles and compresses each.
    fn encode(&self, p: &Page) -> Vec<Vec<u8>> {
        let pix = p.pix.as_deref().unwrap_or_default();
        let rb = (p.w * p.spp * p.bps).div_ceil(8);
        let mut out = Vec::new();
        if p.tile_w > 0 {
            let trb = (p.tile_w * p.spp * p.bps).div_ceil(8);
            for y in (0..p.h).step_by(p.tile_l) {
                for x in (0..p.w).step_by(p.tile_w) {
                    let mut rows = vec![vec![0u8; trb]; p.tile_l];
                    for (r, row) in rows.iter_mut().enumerate() {
                        if y + r < p.h {
                            let a = (y + r) * rb + x * trb / p.tile_w;
                            let src = &pix[a..(y + r + 1) * rb];
                            let n = trb.min(src.len());
                            row[..n].copy_from_slice(&src[..n]);
                        }
                    }
                    out.push(self.compress(p, rows, p.tile_w));
                }
            }
            return out;
        }
        let rps = if p.rows_per_strip == 0 {
            p.h
        } else {
            p.rows_per_strip
        };
        for y in (0..p.h).step_by(rps) {
            let rows = (y..(y + rps).min(p.h))
                .map(|r| pix[r * rb..(r + 1) * rb].to_vec())
                .collect();
            out.push(self.compress(p, rows, p.w));
        }
        out
    }

    fn compress(&self, p: &Page, rows: Vec<Vec<u8>>, cw: usize) -> Vec<u8> {
        if p.compression == 7 {
            return jpeg_chunk(p, &rows, cw);
        }
        let mut raw = Vec::new();
        for mut r in rows {
            if p.bps == 16 && self.le {
                for i in (0..r.len() - 1).step_by(2) {
                    r.swap(i, i + 1);
                }
            }
            if p.predictor == 2 {
                for i in (p.spp..cw * p.spp).rev() {
                    if p.bps == 8 {
                        r[i] = r[i].wrapping_sub(r[i - p.spp]);
                    } else {
                        let rd = |b: &[u8]| {
                            let a = [b[0], b[1]];
                            if self.le {
                                u16::from_le_bytes(a)
                            } else {
                                u16::from_be_bytes(a)
                            }
                        };
                        let v = rd(&r[2 * i..]).wrapping_sub(rd(&r[2 * (i - p.spp)..]));
                        let a = if self.le {
                            v.to_le_bytes()
                        } else {
                            v.to_be_bytes()
                        };
                        r[2 * i..2 * i + 2].copy_from_slice(&a);
                    }
                }
            }
            raw.extend(r);
        }
        match p.compression {
            5 => lzw_encode(&raw),
            8 | 32946 => zlib_store(&raw),
            32773 => pack_bits(&raw),
            _ => raw,
        }
    }
}

fn jpeg_chunk(p: &Page, rows: &[Vec<u8>], cw: usize) -> Vec<u8> {
    let cancel = AtomicBool::new(false);
    let o = jpeg::EncodeOptions {
        quality: 95,
        chroma444: true,
    };
    let l = jpeg::Limits::default();
    let rect = jpeg::Rect::new(0, 0, cw as i32, rows.len() as i32);
    if p.spp == 1 {
        let pix = rows.concat();
        let g = jpeg::Gray {
            pix,
            stride: cw,
            rect,
        };
        return jpeg::encode(jpeg::EncodeImage::Gray(&g), &o, &l, &cancel).unwrap();
    }
    let mut pix = vec![255u8; 4 * cw * rows.len()];
    for (y, r) in rows.iter().enumerate() {
        for x in 0..cw {
            pix[y * 4 * cw + 4 * x..][..3].copy_from_slice(&r[3 * x..3 * x + 3]);
        }
    }
    let m = jpeg::Rgba {
        pix,
        stride: 4 * cw,
        rect,
    };
    jpeg::encode(jpeg::EncodeImage::Rgba(&m), &o, &l, &cancel).unwrap()
}

pub fn golden(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/golden")
            .join(name),
    )
    .unwrap()
}

/// Strips and tiles of every codec, over several chunks each, written by the fixture writer, and
/// libtiff's files (CCITT among them) from testdata.
pub fn fixtures() -> Vec<(String, Vec<u8>)> {
    let mut v = Vec::new();
    for comp in [1, 5, 8, 32773, 7] {
        for spp in [1, 3] {
            for tiles in [false, true] {
                let (w, h) = (96, 80);
                let mut p = Page {
                    w,
                    h,
                    spp,
                    bps: 8,
                    // jpeg-rs writes YCbCr: Photometric RGB would read it as RGB as stored.
                    photometric: match (spp, comp) {
                        (1, _) => 1,
                        (_, 7) => 6,
                        _ => 2,
                    },
                    compression: comp,
                    pix: Some(pattern(w, h, spp, 8)),
                    ..Default::default()
                };
                if tiles {
                    (p.tile_w, p.tile_l) = (32, 16);
                } else {
                    p.rows_per_strip = 8;
                }
                let name = format!("c{comp}_spp{spp}_{}", if tiles { "tile" } else { "strip" });
                let b = File {
                    pages: vec![p],
                    ..Default::default()
                }
                .bytes();
                v.push((name, b));
            }
        }
    }
    for n in [
        "bw_g4_r16.tif",
        "bw_g3pfill_r16.tif",
        "grey_zip_strip.tif",
        "rgb_lzw_tile.tif",
        "g16_zip-p2_strip.tif",
        "grey_jpeg_strip.tif",
        "rgb_jpeg_tile.tif",
    ] {
        v.push((n.to_string(), golden(n)));
    }
    v
}

/// The `max_alloc` values: the default, room for a few workers, for about one, and refusals.
pub const ALLOCS: [u64; 8] = [
    1 << 30,
    4 << 20,
    1 << 20,
    300_000,
    120_000,
    40_000,
    3_000,
    99,
];

/// One 1024x512 RGB strip of JPEG: large enough (two stripes of 131072 pixels and more) for
/// jpeg-rs to thread its colour conversion, and big enough for heap measures.
pub fn big_jpeg() -> Vec<u8> {
    jpeg_1024x512(0)
}

/// The same image in 8 strips of 64 rows: a pipeline of chunks (each under the 131072 pixels
/// that thread jpeg-rs's colour conversion, so the threads are tiff's own).
pub fn big_jpeg_strips() -> Vec<u8> {
    jpeg_1024x512(64)
}

fn jpeg_1024x512(rows_per_strip: usize) -> Vec<u8> {
    let (w, h) = (1024, 512);
    File {
        pages: vec![Page {
            w,
            h,
            spp: 3,
            bps: 8,
            photometric: 6,
            compression: 7,
            rows_per_strip,
            pix: Some(pattern(w, h, 3, 8)),
            ..Default::default()
        }],
        ..Default::default()
    }
    .bytes()
}

/// The packed RGB `rgb` of `w` by `h` pixels as jpeg-rs encodes it: SOI, JFIF APP0, YCbCr with
/// component ids 1, 2, 3, 4:4:4 or 4:2:0.
pub fn ycc_jpeg(rgb: &[u8], w: usize, h: usize, chroma444: bool) -> Vec<u8> {
    let mut pix = vec![255u8; 4 * w * h];
    for (d, s) in pix
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(rgb.as_chunks::<3>().0)
    {
        d[..3].copy_from_slice(s);
    }
    let m = jpeg::Rgba {
        pix,
        stride: 4 * w,
        rect: jpeg::Rect::new(0, 0, w as i32, h as i32),
    };
    let o = jpeg::EncodeOptions {
        quality: 90,
        chroma444,
    };
    let l = jpeg::Limits::default();
    jpeg::encode(jpeg::EncodeImage::Rgba(&m), &o, &l, &AtomicBool::new(false)).unwrap()
}

/// An Adobe APP14 segment with transform 1: YCbCr.
pub const ADOBE_YCC: [u8; 16] = [
    0xff, 0xee, 0, 14, b'A', b'd', b'o', b'b', b'e', 0, 100, 0, 0, 0, 0, 1,
];

/// The stream `j` of [`ycc_jpeg`] with its colour markers varied: as encoded (JFIF), with none
/// (as Aperio writes its tiles), and with an Adobe APP14 transform 1 after the SOI as well.
pub fn marked(j: &[u8]) -> Vec<(&'static str, Vec<u8>)> {
    assert_eq!(j[2..11], [0xff, 0xe0, 0, 16, b'J', b'F', b'I', b'F', 0]);
    let mut none = j[..2].to_vec();
    none.extend_from_slice(&j[20..]);
    let mut adobe = j[..2].to_vec();
    adobe.extend_from_slice(&ADOBE_YCC);
    adobe.extend_from_slice(&j[2..]);
    vec![
        ("jfif", j.to_vec()),
        ("none", none),
        ("jfif and adobe 1", adobe),
    ]
}

/// The packed RGB jpeg-rs gives for the stream `j` read with `color_model` (alpha dropped).
pub fn jpeg_rgb(j: &[u8], color_model: jpeg::ColorModel) -> Vec<u8> {
    let o = jpeg::DecodeOptions {
        format: jpeg::Format::Rgba,
        color_model,
        ..Default::default()
    };
    let d = jpeg::decode(j, &o, &jpeg::Limits::default(), &AtomicBool::new(false)).unwrap();
    assert_eq!(d.error, None);
    let jpeg::Image::Rgba(m) = d.image else {
        panic!("not rgba")
    };
    let mut v = Vec::new();
    for y in m.rect.min_y..m.rect.max_y {
        let o = m.pix_offset(m.rect.min_x, y);
        for x in 0..m.rect.width() as usize {
            v.extend_from_slice(&m.pix[o + 4 * x..o + 4 * x + 3]);
        }
    }
    v
}

/// A zlib stream of stored blocks.
pub fn zlib_store(raw: &[u8]) -> Vec<u8> {
    let mut o = vec![0x78, 0x01];
    let mut chunks: Vec<&[u8]> = raw.chunks(65535).collect();
    if chunks.is_empty() {
        chunks.push(&[]);
    }
    let last = chunks.len() - 1;
    for (i, c) in chunks.into_iter().enumerate() {
        o.push(u8::from(i == last));
        o.extend((c.len() as u16).to_le_bytes());
        o.extend((!(c.len() as u16)).to_le_bytes());
        o.extend(c);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &v in raw {
        a = (a + u32::from(v)) % 65521;
        b = (b + a) % 65521;
    }
    o.extend(((b << 16) | a).to_be_bytes());
    o
}

pub fn pack_bits(b: &[u8]) -> Vec<u8> {
    let mut o = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let mut n = 1;
        while i + n < b.len() && b[i + n] == b[i] && n < 128 {
            n += 1;
        }
        if n >= 3 {
            o.push((257 - n) as u8);
            o.push(b[i]);
            i += n;
            continue;
        }
        let mut j = i;
        while j < b.len()
            && j - i < 128
            && (j + 2 >= b.len() || b[j] != b[j + 1] || b[j] != b[j + 2])
        {
            j += 1;
        }
        if j == i {
            j += 1;
        }
        o.push((j - i - 1) as u8);
        o.extend(&b[i..j]);
        i = j;
    }
    o
}

struct Lzw {
    out: Vec<u8>,
    acc: u32,
    nb: u32,
    width: u32,
    next: usize,
}

impl Lzw {
    fn emit(&mut self, c: usize) {
        self.acc = (self.acc << self.width) | c as u32;
        self.nb += self.width;
        while self.nb >= 8 {
            self.nb -= 8;
            self.out.push((self.acc >> self.nb) as u8);
        }
    }

    fn grow(&mut self) {
        if self.next >= 1 << self.width && self.width < 12 {
            self.width += 1;
        }
    }
}

pub fn lzw_encode(b: &[u8]) -> Vec<u8> {
    use std::collections::HashMap;
    let mut e = Lzw {
        out: Vec::new(),
        acc: 0,
        nb: 0,
        width: 9,
        next: 258,
    };
    let mut dict: HashMap<u32, usize> = HashMap::new();
    e.emit(256);
    let mut w: Option<usize> = None;
    for &c in b {
        let Some(cur) = w else {
            w = Some(usize::from(c));
            continue;
        };
        if let Some(&code) = dict.get(&((cur as u32) << 8 | u32::from(c))) {
            w = Some(code);
            continue;
        }
        e.grow();
        e.emit(cur);
        dict.insert((cur as u32) << 8 | u32::from(c), e.next);
        e.next += 1;
        w = Some(usize::from(c));
        if e.next == 4094 {
            e.emit(256);
            dict.clear();
            e.width = 9;
            e.next = 258;
        }
    }
    if let Some(cur) = w {
        e.grow();
        e.emit(cur);
        e.next += 1;
    }
    e.grow();
    e.emit(257);
    if e.nb > 0 {
        e.out.push((e.acc << (8 - e.nb)) as u8);
    }
    e.out
}

/// Reassembles page `i` and reports the largest band handed out.
pub fn decode(b: &[u8], lim: &Limits, i: usize) -> Result<(Vec<u8>, usize), Error> {
    let f = tiff::File::open(b, b.len() as u64, lim)?;
    let p = f.page(i)?;
    let rb = p.info().row_bytes as usize;
    let (mut pix, mut max_band, mut next) = (Vec::new(), 0, 0);
    p.rows(&AtomicBool::new(false), |y, rows| {
        assert_eq!(y, next, "band starts at row {y}, want {next}");
        max_band = max_band.max(rows.len());
        pix.extend_from_slice(rows);
        next += rows.len() / rb;
        ControlFlow::Continue(())
    })?;
    Ok((pix, max_band))
}

pub fn lim() -> Limits {
    Limits::default()
}

pub type Bands = Vec<(usize, Vec<u8>)>;

/// Every band `(y, bytes)` of page 0, the error that ended the decode (an open or page error
/// included) and what the call used.
pub fn collect<R: tiff::ReadAt>(
    ra: R,
    size: u64,
    lim: &Limits,
    pool: Option<&tiff::ThreadPool>,
) -> (Bands, Option<Error>, tiff::Probe) {
    let none = tiff::Probe {
        charged_peak: 0,
        threads_peak: 1,
    };
    let mut bands = Vec::new();
    let f = match tiff::File::open(ra, size, lim) {
        Ok(f) => f,
        Err(e) => return (bands, Some(e), none),
    };
    let f = match pool {
        Some(p) => f.with_pool(p.clone()),
        None => f,
    };
    let page = match f.page(0) {
        Ok(p) => p,
        Err(e) => return (bands, Some(e), none),
    };
    let (r, probe) = page.rows_probe(&AtomicBool::new(false), |y, b| {
        bands.push((y, b.to_vec()));
        ControlFlow::Continue(())
    });
    (bands, r.err(), probe)
}
