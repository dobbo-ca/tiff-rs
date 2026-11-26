//! JPEG strips and tiles. A baseline frame is decoded a band of rows at a time through the
//! jpeg-rs region decode; any other frame (progressive, non-interleaved) through one
//! `jpeg::RowReader`, which holds the frame's coefficients. The compressed chunk is held whole
//! (jpeg-rs takes a slice); decoded pixels are never held beyond one band. The reader decodes
//! through the thread's `jpeg::Decoder`, which keeps its planes from band to band and chunk to
//! chunk; the chunk's bytes and the band copy are vectors the thread keeps too, so a chunk no
//! larger than an earlier one allocates nothing. Each use books their capacity, not only what the
//! chunk needs. A progressive frame is the exception: the
//! `jpeg::RowReader` builds its own buffers on every call. Ported from tiff-go's
//! `internal/codec/jpeg.go`.

use super::{Env, Params, RowReader, Rows, check, jpeg4};
use crate::error::{Error, corrupt, limit, unsupported};
use crate::ledger::Held;
use crate::readat::{ReadAt, SectionReader};
use std::borrow::Cow;
use std::sync::atomic::AtomicBool;

/// Bounds one jpeg-rs call: its planes and conversion buffers run to a few times the decoded
/// band, and its allocation is charged in 8 KiB pages.
pub(super) fn jpeg_budget(band: u64) -> u64 {
    band.saturating_mul(4).saturating_add(1 << 20)
}

/// Maps a jpeg-rs failure: a refused `Limits` is a limit, the rest is corruption.
fn jpeg_err(e: jpeg::Error, what: &str) -> Error {
    match e {
        jpeg::Error::PixelLimit { got, max } => limit("jpeg pixels", got, max),
        jpeg::Error::AllocLimit { got, max } => limit("jpeg alloc", got, max),
        jpeg::Error::OutOfMemory => Error::OutOfMemory,
        jpeg::Error::Cancelled => Error::Cancelled,
        _ => corrupt(what),
    }
}

/// Decodes a strip or tile a band of rows at a time.
pub(crate) struct JpegRows<'a, 'l> {
    cancel: &'a AtomicBool,
    env: &'a mut Env<'l>,
    /// The chunk's bytes, a vector of the thread's `State` that goes back there on drop, and the
    /// charge for them.
    src: Vec<u8>,
    _src_held: Held<'a>,
    tables: &'a [u8],
    format: jpeg::Format,
    color_model: jpeg::ColorModel,
    limits: jpeg::Limits,
    w: usize,
    h: usize,
    band_rows: usize,
    /// The next row to hand out.
    next: usize,
    /// The last band's rows as delivered, packed; the vector is the thread's and goes back on
    /// drop. The decoder's own image is only valid until its next call, so the band is copied out; the copy is at most `band_rows` rows, which
    /// `max_alloc` of the jpeg-rs call leaves out of the reservation booked for both.
    rows: Vec<u8>,
    /// The capacity of `rows` beyond the reservation, booked until the reader drops.
    _rows_extra: Option<Held<'a>>,
    band_start: usize,
    band_end: usize,
}

fn tables_of(t: &[u8]) -> Option<&[u8]> {
    if t.is_empty() { None } else { Some(t) }
}

pub(super) fn new<'a, 'l: 'a, R: ReadAt>(
    src: SectionReader<'a, R>,
    p: &Params<'a>,
    cancel: &'a AtomicBool,
    env: &'a mut Env<'l>,
) -> Result<Rows<'a, 'l, R>, Error> {
    if p.spp == 4 {
        return jpeg4::new(
            src,
            p.jpeg_tables,
            p.w,
            p.h,
            p.band_bytes,
            p.alloc_left,
            cancel,
            env,
        )
        .map(Rows::Boxed);
    }
    if p.spp != 1 && p.spp != 3 {
        return Err(unsupported(format!("jpeg with {} samples", p.spp)));
    }
    let mut left = p.alloc_left;
    let (mut b, mut b_held) = env.read_chunk(src, &mut left, cancel)?;
    let tables = tables_of(p.jpeg_tables);
    let band = bandable(&b, cancel)?;
    let cfg = jpeg::decode_config(&b, tables, &jpeg::Limits::default())
        .map_err(|e| jpeg_err(e, "jpeg header"))?;
    if cfg.width != p.w || cfg.height != p.h || cfg.components != p.spp {
        return Err(corrupt("jpeg frame does not match its chunk"));
    }
    let color_model = if p.jpeg_rgb && p.spp == 3 {
        // libtiff refuses an RGB tile whose components are subsampled ("Improper JPEG sampling
        // factors"); jpeg-rs too.
        if cfg.ratio != jpeg::Subsample::S444 {
            return Err(unsupported(
                "jpeg with photometric rgb and subsampled components",
            ));
        }
        jpeg::ColorModel::Rgb
    } else {
        jpeg::ColorModel::Auto
    };
    let (format, px) = if p.spp == 1 {
        (jpeg::Format::Gray, 1u64)
    } else {
        (jpeg::Format::Rgba, 4) // decoded bytes per pixel
    };
    if px * p.w as u64 > p.band_bytes {
        return Err(limit("band bytes", px * p.w as u64, p.band_bytes));
    }
    if !band {
        // jpeg-rs would decode the whole frame for every band: hold the coefficients once and
        // take rows from a reader, whose need it checks before it allocates.
        let need = jpeg::row_reader_need(&b, tables, &jpeg::Limits::default())
            .map_err(|e| jpeg_err(e, "jpeg header"))?;
        // The ledger of jpeg-rs also counts page rounding, under 128 KiB.
        let max_alloc = need.saturating_add(1 << 17);
        // The copy of one band kept between calls: at most 8 * vmax = 32 rows of RGBA.
        let band_cap = 4 * 32 * p.w as u64;
        let total = max_alloc.saturating_add(band_cap);
        if total > left {
            return Err(limit("alloc", total, left));
        }
        if b.capacity() > b.len() {
            // Capacity kept from a larger chunk: a cold call has none, so give the spare back
            // before booking the reader (a warm Decoder must fail where a cold one fails).
            let was = b.capacity();
            b.shrink_to_fit();
            b_held.release((was - b.capacity()) as u64);
        }
        let held = env.charge(total)?;
        let lim = jpeg::Limits {
            max_alloc,
            max_threads: 1,
            ..Default::default()
        };
        let rr = jpeg::RowReader::with_color_model(&b, tables, color_model, &lim, cancel)
            .map_err(|e| jpeg_err(e, "jpeg data"))?;
        if rr.truncated() {
            return Err(corrupt("jpeg data"));
        }
        let mut pix = Vec::new();
        pix.try_reserve_exact(band_cap as usize)
            .map_err(|_| Error::OutOfMemory)?;
        // The reader holds its own copy of what it needs: the chunk's bytes go back to the thread,
        // still booked until the reader drops (the pipeline books them again after the job).
        env.give_src(b);
        return Ok(Rows::Boxed(Box::new(JpegProgRows {
            rr,
            cancel,
            cur: jpeg::Rgba {
                pix,
                stride: 0,
                rect: jpeg::Rect::new(0, 0, 0, 0),
            },
            w: p.w,
            h: p.h,
            next: 0,
            gray: p.spp == 1,
            _held: held,
            _src_held: b_held,
        })));
    }
    // What a serial call checks, and what this call books: the same unless the chunk is decoded
    // whole on a worker, which needs a budget for the chunk only.
    let budget = jpeg_budget(p.jpeg_bytes);
    let mut reserved = env.reserve_decoder(jpeg_budget(p.band_bytes), left, budget);
    if matches!(reserved, Err(Error::Limit { .. })) && b.capacity() > b.len() {
        // The chunk's buffer kept a capacity from a larger chunk, booked beside the reservation: a
        // cold call has no such buffer, so give the spare back and try once more.
        let was = b.capacity();
        b.shrink_to_fit();
        b_held.release((was - b.capacity()) as u64);
        reserved = env.reserve_decoder(jpeg_budget(p.band_bytes), left, budget);
    }
    reserved?;
    let band_rows = (p.h as u64).min(p.band_bytes / (px * p.w as u64)) as usize;
    // The band copy (`rows`) lives in the reservation, so jpeg-rs gets the rest of it: the copy
    // is at most a band of delivered rows, under the 4 bands and a megabyte it is budgeted, and
    // jpeg-rs needs about 1.75 bands.
    let bytes_px: u64 = if p.spp == 1 { 1 } else { 3 };
    let band_cap = band_rows as u64 * bytes_px * p.w as u64;
    let limits = jpeg::Limits {
        max_alloc: budget.saturating_sub(band_cap),
        max_threads: env.jpeg_threads,
        ..Default::default()
    };
    let mut rows = std::mem::take(&mut env.st.jrows);
    env.release_idle(rows.capacity());
    // A band copy kept from a larger chunk is booked beyond the reservation, or freed. It is
    // booked on the ledger itself: dropping the caches would drop this chunk's own reservation.
    let extra = (rows.capacity() as u64).saturating_sub(band_cap);
    let rows_extra = if extra == 0 {
        None
    } else {
        match env.ledger.charge(extra) {
            Ok(h) => Some(h),
            Err(_) => {
                rows = Vec::new();
                None
            }
        }
    };
    Ok(Rows::Jpeg(JpegRows {
        cancel,
        env,
        src: b,
        _src_held: b_held,
        tables: p.jpeg_tables,
        format,
        color_model,
        limits,
        w: p.w,
        h: p.h,
        band_rows,
        next: 0,
        rows,
        _rows_extra: rows_extra,
        band_start: 0,
        band_end: 0,
    }))
}

impl Drop for JpegRows<'_, '_> {
    fn drop(&mut self) {
        // The buffers go back to the thread's state for its next chunk.
        self.env.st.src = std::mem::take(&mut self.src);
        self.env.st.jrows = std::mem::take(&mut self.rows);
    }
}

/// Hands out a frame's rows from one `jpeg::RowReader`, which holds the frame's coefficients
/// until it is dropped.
struct JpegProgRows<'a> {
    rr: jpeg::RowReader,
    /// The reader's coefficients and the band copy, booked until it drops.
    _held: Held<'a>,
    /// The booking of the chunk's bytes, which went back to the thread's state.
    _src_held: Held<'a>,
    cancel: &'a AtomicBool,
    /// The last band, copied (the reader reuses its own) into a buffer reserved once.
    cur: jpeg::Rgba,
    w: usize,
    h: usize,
    next: usize,
    gray: bool,
}

impl RowReader for JpegProgRows<'_> {
    fn read_row(&mut self, dst: &mut [u8]) -> Result<(), Error> {
        if self.next >= self.h {
            return Err(corrupt("jpeg rows"));
        }
        if self.next as i32 >= self.cur.rect.max_y {
            match self
                .rr
                .next(self.cancel)
                .map_err(|e| jpeg_err(e, "jpeg data"))?
            {
                Some(m) => {
                    self.cur.pix.clear();
                    self.cur.pix.extend_from_slice(&m.pix);
                    self.cur.stride = m.stride;
                    self.cur.rect = m.rect;
                }
                None => return Err(corrupt("jpeg data")),
            }
        }
        copy_rgba(&self.cur, self.next, self.w, self.gray, dst)?;
        self.next += 1;
        Ok(())
    }
}

/// Row `y` of `m` as grey or RGB samples.
fn copy_rgba(m: &jpeg::Rgba, y: usize, w: usize, gray: bool, dst: &mut [u8]) -> Result<(), Error> {
    let o = m.pix_offset(m.rect.min_x, y as i32);
    let Some(s) = m.pix.get(o..o + 4 * w) else {
        return Err(corrupt("jpeg output"));
    };
    if gray {
        for i in 0..w {
            dst[i] = s[4 * i];
        }
    } else {
        for i in 0..w {
            dst[3 * i..3 * i + 3].copy_from_slice(&s[4 * i..4 * i + 3]);
        }
    }
    Ok(())
}

impl JpegRows<'_, '_> {
    /// Decodes the band from `self.next` to `end` and returns it, or the band's first error. It
    /// borrows the `Env` alone, so the caller may use the other fields beside the image. A probe
    /// call decodes through `jpeg::decode_probe` and notes the call's threads in the gauge.
    fn decode_band<'d>(
        env: &'d mut Env<'_>,
        src: &[u8],
        o: &jpeg::DecodeOptions,
        limits: &jpeg::Limits,
        cancel: &AtomicBool,
    ) -> Result<Cow<'d, jpeg::Decoded>, Error> {
        let d = if let Some(g) = env.probe {
            let o = jpeg::DecodeOptions {
                pool: env.jpeg_pool.as_ref(),
                ..*o
            };
            let (r, pr) = jpeg::decode_probe(src, &o, limits, cancel, false);
            g.note(pr.threads_peak);
            Cow::Owned(r.map_err(|e| jpeg_err(e, "jpeg data"))?)
        } else {
            let d = env
                .st
                .dec
                .decode(src, o, limits, cancel)
                .map_err(|e| jpeg_err(e, "jpeg data"))?;
            Cow::Borrowed(d)
        };
        if let Some(e) = d.error.clone() {
            return Err(jpeg_err(e, "jpeg data"));
        }
        Ok(d)
    }
}

/// The options of the band `y0..y1` of a `w` by `h` frame decoded `band_rows` rows at a time.
fn band_options(
    (format, color_model): (jpeg::Format, jpeg::ColorModel),
    tables: &[u8],
    (w, h, band_rows): (usize, usize, usize),
    (y0, y1): (usize, usize),
) -> jpeg::DecodeOptions<'_> {
    let region = if band_rows < h {
        Some(jpeg::Rect::new(0, y0 as i32, w as i32, y1 as i32))
    } else {
        None
    };
    jpeg::DecodeOptions {
        format,
        color_model,
        region,
        tables: tables_of(tables),
        ..Default::default()
    }
}

/// Copies rows `y0..` of a decoded band into `out`, `out.len() / rbj` of them, `rbj` bytes each.
fn copy_rows(
    img: &jpeg::Image,
    w: usize,
    y0: usize,
    out: &mut [u8],
    rbj: usize,
) -> Result<(), Error> {
    for (k, row) in out.chunks_exact_mut(rbj).enumerate() {
        let y = y0 + k;
        match img {
            jpeg::Image::Rgba(m) => copy_rgba(m, y, w, false, row)?,
            jpeg::Image::Gray(m) => {
                let o = m.pix_offset(m.rect.min_x, y as i32);
                let Some(s) = m.pix.get(o..o + w) else {
                    return Err(corrupt("jpeg output"));
                };
                row.copy_from_slice(s);
            }
            _ => return Err(corrupt("jpeg output")),
        }
    }
    Ok(())
}

impl RowReader for JpegRows<'_, '_> {
    fn read_row(&mut self, dst: &mut [u8]) -> Result<(), Error> {
        if self.next >= self.h {
            return Err(corrupt("jpeg rows"));
        }
        let rbj = dst.len();
        if self.next >= self.band_end {
            // Release the last band before the next decode, so two are never held at once.
            self.rows.clear();
            check(self.cancel)?;
            let end = (self.next + self.band_rows).min(self.h);
            let o = band_options(
                (self.format, self.color_model),
                self.tables,
                (self.w, self.h, self.band_rows),
                (self.next, end),
            );
            let d = Self::decode_band(self.env, &self.src, &o, &self.limits, self.cancel)?;
            let n = (end - self.next) * rbj;
            self.rows
                .try_reserve_exact(n)
                .map_err(|_| Error::OutOfMemory)?;
            self.rows.resize(n, 0);
            copy_rows(&d.image, self.w, self.next, &mut self.rows, rbj)?;
            self.band_start = self.next;
            self.band_end = end;
        }
        let o = (self.next - self.band_start) * rbj;
        dst.copy_from_slice(&self.rows[o..o + rbj]);
        self.next += 1;
        Ok(())
    }

    /// As `read_row` for each row of `dst`; a band that `dst` takes whole is copied straight out
    /// of the decoder's image instead of through `self.rows`.
    fn read_rows(&mut self, mut dst: &mut [u8], rbj: usize) -> Result<(), Error> {
        while !dst.is_empty() {
            let end = (self.next + self.band_rows).min(self.h);
            if self.next < self.band_end
                || self.next >= self.h
                || dst.len() < (end - self.next) * rbj
            {
                let (row, rest) = dst.split_at_mut(rbj);
                self.read_row(row)?;
                dst = rest;
                continue;
            }
            self.rows.clear();
            check(self.cancel)?;
            let o = band_options(
                (self.format, self.color_model),
                self.tables,
                (self.w, self.h, self.band_rows),
                (self.next, end),
            );
            let d = Self::decode_band(self.env, &self.src, &o, &self.limits, self.cancel)?;
            let n = (end - self.next) * rbj;
            copy_rows(&d.image, self.w, self.next, &mut dst[..n], rbj)?;
            self.next = end;
            dst = &mut dst[n..];
        }
        Ok(())
    }
}

/// The bytes scanned between two cancel checks.
const SCAN_BLOCK: usize = 1 << 20;

/// Reports whether jpeg-rs can decode `b` a band at a time: a baseline frame that is
/// single-component or whose first scan carries every component (the same gate as jpeg-rs;
/// otherwise each band re-decodes the whole frame). Markers are found as jpeg-rs's
/// `find_marker` does: bytes between segments are skipped, and TEM and RSTn have no length. Any
/// other reading would let a file show this gate one frame and the decoder another.
fn bandable(b: &[u8], cancel: &AtomicBool) -> Result<bool, Error> {
    let mut n = 0usize; // frame components, 0 until SOF is seen
    let mut i = 2usize;
    let mut blk = 0usize; // the 1 MiB block of `i` at the last cancel check
    loop {
        if i / SCAN_BLOCK != blk {
            blk = i / SCAN_BLOCK;
            check(cancel)?;
        }
        let mut m = 0u8;
        while m == 0 {
            while i < b.len() && b[i] != 0xff {
                i += 1;
                if i & (SCAN_BLOCK - 1) == 0 {
                    check(cancel)?;
                }
            }
            while i < b.len() && b[i] == 0xff {
                i += 1;
                if i & (SCAN_BLOCK - 1) == 0 {
                    check(cancel)?;
                }
            }
            if i >= b.len() {
                return Ok(false);
            }
            m = b[i];
            i += 1;
        }
        if m == 0x01 || (0xd0..=0xd7).contains(&m) {
            continue;
        }
        if m == 0xd8 || m == 0xd9 || i + 2 > b.len() {
            return Ok(false); // duplicate SOI, EOI, truncated
        }
        let mut l = usize::from(b[i]) << 8 | usize::from(b[i + 1]);
        if l < 2 && ((0xe0..=0xef).contains(&m) || m == 0xfe || m == 0xdc) {
            l = 2;
        }
        // jpeg-rs reads a short SOS as is
        if l < 2 || (i + l > b.len() && m != 0xda) {
            return Ok(false);
        }
        match m {
            0xc0 | 0xc1 => {
                // baseline / extended sequential
                if l < 8 {
                    return Ok(false);
                }
                n = usize::from(b[i + 7]);
                if n == 0 {
                    return Ok(false);
                }
            }
            0xc2..=0xcf if m != 0xc4 && m != 0xc8 && m != 0xcc => {
                return Ok(false); // progressive, lossless, arithmetic
            }
            0xda => {
                // first scan
                return Ok(n == 1 || n > 0 && i + 2 < b.len() && usize::from(b[i + 2]) == n);
            }
            _ => {}
        }
        i += l;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::Ledger;

    // Every jpeg-rs call is bounded by the band budget, never run with the default limits.
    #[test]
    fn jpeg_call_limited() {
        let cancel = AtomicBool::new(false);
        let g = jpeg::Gray {
            pix: vec![0; 64 * 64],
            stride: 64,
            rect: jpeg::Rect::new(0, 0, 64, 64),
        };
        let b = jpeg::encode(
            jpeg::EncodeImage::Gray(&g),
            &jpeg::EncodeOptions {
                quality: 0,
                chroma444: false,
            },
            &jpeg::Limits::default(),
            &cancel,
        )
        .unwrap();
        let p = Params {
            compression: 7,
            t4: 0,
            spp: 1,
            bps: 8,
            w: 64,
            h: 64,
            jpeg_tables: &[],
            jpeg_rgb: false,
            band_bytes: 64 * 64,
            jpeg_bytes: 64 * 64,
            alloc_left: 1 << 30,
        };
        let sec = SectionReader::new(&b, 0, b.len() as u64);
        let ledger = Ledger::new(1 << 30);
        let mut env = Env::new(&ledger, 1);
        let r = new(sec, &p, &cancel, &mut env);
        assert!(r.is_ok());
        assert_eq!(jpeg_budget(64 * 64), 4 * 64 * 64 + (1 << 20));
    }

    // The last band is dropped before the next decode runs, so it is never held beside it.
    #[test]
    fn previous_band_released_before_decode() {
        let cancel = AtomicBool::new(false);
        let g = jpeg::Gray {
            pix: vec![0; 64 * 64],
            stride: 64,
            rect: jpeg::Rect::new(0, 0, 64, 64),
        };
        let b = jpeg::encode(
            jpeg::EncodeImage::Gray(&g),
            &jpeg::EncodeOptions {
                quality: 0,
                chroma444: false,
            },
            &jpeg::Limits::default(),
            &cancel,
        )
        .unwrap();
        let ledger = Ledger::new(1 << 30);
        let mut env = Env::new(&ledger, 1);
        let mut left = 1 << 30;
        let (mut src, src_held) = env.alloc(b.len() as u64, &mut left).unwrap().into_parts();
        src.copy_from_slice(&b);
        let mut r = JpegRows {
            cancel: &cancel,
            env: &mut env,
            src,
            _src_held: src_held,
            tables: &[],
            format: jpeg::Format::Gray,
            color_model: jpeg::ColorModel::Auto,
            limits: jpeg::Limits::default(),
            w: 64,
            h: 64,
            band_rows: 16,
            next: 0,
            rows: Vec::new(),
            _rows_extra: None,
            band_start: 0,
            band_end: 0,
        };
        let mut row = [0u8; 64];
        r.read_row(&mut row).unwrap();
        assert!(!r.rows.is_empty());
        r.next = 16;
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(matches!(r.read_row(&mut row), Err(Error::Cancelled)));
        assert!(r.rows.is_empty());
    }

    #[test]
    fn bandable_cases() {
        let sof = |m: u8, n: usize| {
            let mut v = vec![0xff, m, 0, 8 + 3 * n as u8, 8, 0, 8, 0, 8, n as u8];
            v.extend(vec![0; 3 * n]);
            v
        };
        let sos = |n: u8| vec![0xff, 0xda, 0, 6, n];
        let cat = |parts: &[Vec<u8>]| {
            let mut b = vec![0xff, 0xd8];
            for p in parts {
                b.extend(p);
            }
            b
        };
        let cases: Vec<(&str, Vec<u8>, bool)> = vec![
            ("baseline interleaved", cat(&[sof(0xc0, 3), sos(3)]), true),
            ("baseline gray", cat(&[sof(0xc0, 1), sos(1)]), true),
            (
                "baseline non-interleaved",
                cat(&[sof(0xc0, 3), sos(1)]),
                false,
            ),
            ("progressive", cat(&[sof(0xc2, 3), sos(3)]), false),
            (
                "fill before SOF2",
                cat(&[vec![0xff], sof(0xc2, 3), sos(3)]),
                false,
            ),
            (
                "fill before SOF0",
                cat(&[vec![0xff, 0xff], sof(0xc0, 3), sos(3)]),
                true,
            ),
            (
                "app then baseline",
                cat(&[vec![0xff, 0xe0, 0, 4, 1, 2], sof(0xc0, 3), sos(3)]),
                true,
            ),
            (
                "RST length hides fake frame",
                cat(&[
                    vec![0xff, 0xd0, 0, 6, 0xff, 0xe1, 0, 0x20],
                    sof(0xc0, 3),
                    sos(3),
                    vec![0; 8],
                    sof(0xc2, 3),
                    sos(1),
                ]),
                false,
            ),
            ("no frame", cat(&[sos(1)]), false),
            ("truncated", cat(&[sof(0xc0, 3)[..6].to_vec()]), false),
        ];
        for (name, b, want) in cases {
            assert_eq!(bandable(&b, &AtomicBool::new(false)), Ok(want), "{name}");
        }
    }

    // A refused budget is a limit, not corruption.
    #[test]
    fn jpeg_budget_refusal_is_a_limit() {
        let cancel = AtomicBool::new(false);
        let b = vec![
            0xffu8, 0xd8, 0xff, 0xc2, 0, 17, 8, 8, 0, 8, 0, 3, 1, 0x11, 0, 2, 0x11, 0, 3, 0x11, 0,
            0xff, 0xda, 0, 12, 3, 1, 0, 2, 0, 3, 0, 0, 0, 0,
        ];
        let p = Params {
            compression: 7,
            t4: 0,
            spp: 3,
            bps: 8,
            w: 2048,
            h: 2048,
            jpeg_tables: &[],
            jpeg_rgb: false,
            band_bytes: 4 * 2048 * 2048,
            jpeg_bytes: 4 * 2048 * 2048,
            alloc_left: 1 << 20,
        };
        let sec = SectionReader::new(&b, 0, b.len() as u64);
        let ledger = Ledger::new(1 << 20);
        let mut env = Env::new(&ledger, 1);
        match new(sec, &p, &cancel, &mut env) {
            Err(Error::Limit { .. }) => {}
            Err(e) => panic!("{e}"),
            Ok(_) => panic!("accepted"),
        }
    }
}
