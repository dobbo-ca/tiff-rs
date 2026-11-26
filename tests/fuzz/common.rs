//! Shared by the targets (`#[path = "../common.rs"] mod common;`): the limits they run under, a
//! decode that checks the invariants of every band, and writers for small TIFFs.
#![allow(dead_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tiff::{Error, Info, Limits, Probe, ReadAt};

/// Counts the bytes the process allocates, the analogue of Go's `runtime.MemStats.TotalAlloc`: a
/// `realloc` counts its full new size, as Go's append counts the whole new backing array on
/// growth. `budget` reads it around one input.
struct Counting;

static ALLOCATED: AtomicU64 = AtomicU64::new(0);

// SAFETY: every call is forwarded to `System` unchanged; the counter is a side effect.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(l.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(l.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        ALLOCATED.fetch_add(new as u64, Ordering::Relaxed);
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// tiff-go's FuzzTIFF fails an input over 96 MiB of `TotalAlloc`.
pub const MAX_INPUT_ALLOC: u64 = 96 << 20;
/// FuzzTIFF's 1.5 s is for an uninstrumented build; the fuzz targets run under a sanitizer, so
/// the bound is 5 s per `budget` call. One input may make several budgeted runs (readat 3, threads
/// 7, limits 2), so fuzz.yml sets libFuzzer's `-timeout` per target above runs x 5 s (40 for
/// threads, 16 for readat, 11 for limits, 10 for the rest); a target that adds budgeted runs
/// raises its timeout there too.
pub const MAX_INPUT_TIME: Duration = Duration::from_secs(5);

/// Runs `f` and fails if it allocated more than `MAX_INPUT_ALLOC` bytes or ran longer than
/// `MAX_INPUT_TIME` times `$FUZZ_TIME_SCALE` (1 by default). The
/// harness's own buffers count too, so `f` must not store decoded pixels (see `Out`).
pub fn budget<T>(f: impl FnOnce() -> T) -> T {
    let scale = std::env::var("FUZZ_TIME_SCALE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1u32);
    let (a0, t0) = (ALLOCATED.load(Ordering::Relaxed), Instant::now());
    let r = f();
    let (bytes, took) = (ALLOCATED.load(Ordering::Relaxed) - a0, t0.elapsed());
    assert!(bytes <= MAX_INPUT_ALLOC, "input allocated {bytes} bytes");
    assert!(took <= MAX_INPUT_TIME * scale, "input took {took:?}");
    r
}

/// FuzzTIFF's limits, with a metadata cap and `max_alloc` that keep one input well under the
/// `-malloc_limit_mb=512` of the fuzz run.
pub const TIGHT: Limits = Limits {
    max_ifds: 64,
    max_pixels: 1 << 22,
    max_band_bytes: 1 << 20,
    max_metadata_bytes: 1 << 16,
    max_alloc: 64 << 20,
    max_threads: 1,
};

/// Pages one input decodes.
pub const MAX_PAGES: usize = 8;

/// FNV-1a 64 offset basis, the hash of no bytes.
const FNV_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// What a decode delivered, without the pixels (storing them would count against `budget`, which
/// tiff-go's FuzzTIFF does not): a running FNV-1a 64 of the bytes, recorded after every band.
#[derive(Debug, PartialEq)]
pub struct Out {
    /// `(y, len)` of every band, in order.
    pub split: Vec<(usize, usize)>,
    /// `(bytes so far, hash of them)` after every band.
    pub marks: Vec<(usize, u64)>,
    pub err: Option<Error>,
}

impl Out {
    /// `(bytes delivered, their hash)`.
    pub fn digest(&self) -> (usize, u64) {
        self.marks.last().copied().unwrap_or((0, FNV_BASIS))
    }

    /// Whether the bytes this delivered are a prefix of `want`'s: its digest is none, or one of
    /// `want`'s marks (the band splits may differ).
    pub fn is_prefix_of(&self, want: &Out) -> bool {
        self.marks.is_empty() || want.marks.contains(&self.digest())
    }
}

#[derive(Debug, PartialEq)]
pub enum PageRes {
    Err(Error),
    Ok(Info, Out),
}

#[derive(Debug, PartialEq)]
pub enum Res {
    Open(Error),
    Pages(usize, Vec<PageRes>),
}

/// Decodes a page, stopping with `Break` after `stop_after` bands. Every band is whole rows, at
/// most `max_band_bytes`, and starts where the last one ended; a decode that succeeds delivers
/// exactly `height` rows.
pub fn rows_of<R: ReadAt>(
    p: &tiff::Page<'_, R>,
    lim: &Limits,
    cancel: &AtomicBool,
    stop_after: usize,
) -> Out {
    rows_with(p, lim, cancel, stop_after, usize::MAX)
}

/// `rows_of` that also sets `cancel` from the callback after `cancel_after` bands.
pub fn rows_with<R: ReadAt>(
    p: &tiff::Page<'_, R>,
    lim: &Limits,
    cancel: &AtomicBool,
    stop_after: usize,
    cancel_after: usize,
) -> Out {
    rows_run(p, lim, cancel, stop_after, cancel_after, false).0
}

/// `rows_with` through `Page::rows_probe`, which also returns what the call used. Only the
/// `threads` target uses it: every other one fuzzes the production `Page::rows`.
pub fn rows_probe_with<R: ReadAt>(
    p: &tiff::Page<'_, R>,
    lim: &Limits,
    cancel: &AtomicBool,
    stop_after: usize,
    cancel_after: usize,
) -> (Out, Probe) {
    rows_run(p, lim, cancel, stop_after, cancel_after, true)
}

fn rows_run<R: ReadAt>(
    p: &tiff::Page<'_, R>,
    lim: &Limits,
    cancel: &AtomicBool,
    stop_after: usize,
    cancel_after: usize,
    probe: bool,
) -> (Out, Probe) {
    let n = p.info();
    let rb = n.row_bytes as usize;
    let mut total = 0usize;
    let mut hash = FNV_BASIS;
    let mut split = Vec::new();
    let mut marks = Vec::new();
    let mut bands = 0;
    let each = |y: usize, band: &[u8]| {
        assert!(!band.is_empty(), "empty band");
        assert!(band.len() as u64 <= lim.max_band_bytes, "band over the cap");
        assert!(rb != 0 && band.len() % rb == 0, "band is not whole rows");
        assert_eq!(y * rb, total, "band does not follow the last");
        for &b in band {
            hash = (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        total += band.len();
        split.push((y, band.len()));
        marks.push((total, hash));
        bands += 1;
        if bands >= cancel_after {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if bands >= stop_after {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let (r, probe) = if probe {
        p.rows_probe(cancel, each)
    } else {
        let none = Probe {
            charged_peak: 0,
            threads_peak: 1,
        };
        (p.rows(cancel, each), none)
    };
    if r.is_ok() {
        assert_eq!(total, n.height * rb, "a decode that succeeded is short");
    }
    if r == Err(Error::Stopped) {
        assert!(bands >= stop_after, "stopped without a Break");
    }
    (
        Out {
            split,
            marks,
            err: r.err(),
        },
        probe,
    )
}

/// Opens `ra` and decodes up to `MAX_PAGES` pages, within FuzzTIFF's allocation and time bounds.
pub fn run_file<R: ReadAt>(ra: R, size: u64, lim: &Limits) -> Res {
    budget(|| run_file_inner(ra, size, lim))
}

fn run_file_inner<R: ReadAt>(ra: R, size: u64, lim: &Limits) -> Res {
    let cancel = AtomicBool::new(false);
    let f = match tiff::File::open(ra, size, lim) {
        Ok(f) => f,
        Err(e) => return Res::Open(e),
    };
    let mut pages = Vec::new();
    for i in 0..f.num_pages().min(MAX_PAGES) {
        pages.push(match f.page(i) {
            Ok(p) => {
                let n = p.info();
                assert!((n.width as u64).saturating_mul(n.height as u64) <= lim.max_pixels);
                let o = rows_of(&p, lim, &cancel, usize::MAX);
                PageRes::Ok(n, o)
            }
            Err(e) => PageRes::Err(e),
        });
    }
    Res::Pages(f.num_pages(), pages)
}

pub struct Entry {
    pub tag: u16,
    pub typ: u16,
    pub vals: Vec<u64>,
}

fn width(typ: u16) -> usize {
    match typ {
        3 => 2,
        4 => 4,
        16 => 8,
        _ => 1,
    }
}

fn put(b: &mut Vec<u8>, v: u64, w: usize) {
    b.extend_from_slice(&v.to_le_bytes()[..w]);
}

/// A little-endian TIFF: header, `chunk`, the entries' out-of-line values, one IFD with the
/// entries sorted by tag. A value that fits the value field is inline.
pub fn build(big: bool, chunk: &[u8], mut es: Vec<Entry>) -> Vec<u8> {
    let hdr = if big { 16 } else { 8 };
    let mut b = vec![0u8; hdr];
    b.extend_from_slice(chunk);
    es.sort_by_key(|e| e.tag);
    let inl = if big { 8 } else { 4 };
    let mut fields = Vec::new();
    for e in &es {
        let w = width(e.typ);
        let mut f = Vec::new();
        if e.vals.len() * w <= inl {
            for &v in &e.vals {
                put(&mut f, v, w);
            }
            f.resize(inl, 0);
        } else {
            if b.len() % 2 == 1 {
                b.push(0);
            }
            put(&mut f, b.len() as u64, inl);
            for &v in &e.vals {
                put(&mut b, v, w);
            }
        }
        fields.push(f);
    }
    if b.len() % 2 == 1 {
        b.push(0);
    }
    let ifd = b.len() as u64;
    if big {
        b[..8].copy_from_slice(b"II+\0\x08\0\0\0");
        b[8..16].copy_from_slice(&ifd.to_le_bytes());
        put(&mut b, es.len() as u64, 8);
    } else {
        b[..4].copy_from_slice(b"II*\0");
        b[4..8].copy_from_slice(&(ifd as u32).to_le_bytes());
        put(&mut b, es.len() as u64, 2);
    }
    for (e, f) in es.iter().zip(&fields) {
        put(&mut b, u64::from(e.tag), 2);
        put(&mut b, u64::from(e.typ), 2);
        put(&mut b, e.vals.len() as u64, inl);
        b.extend_from_slice(f);
    }
    put(&mut b, 0, inl);
    b
}

/// `oobTIFF` of tiff-go's skip_test.go: a 1x1 8-bit grey TIFF, classic or BigTIFF, plus extra
/// tags whose type, count and value field are written as given.
pub fn raw_entries(big: bool, extra: &[[u64; 4]]) -> Vec<u8> {
    let mut b: Vec<u8> = if big {
        let mut h = b"II+\0\x08\0\0\0".to_vec();
        h.extend([0u8; 8]);
        h
    } else {
        b"II*\0\0\0\0\0".to_vec()
    };
    let data = b.len() as u64;
    b.push(7);
    let ifd = b.len() as u64;
    if big {
        b[8..16].copy_from_slice(&ifd.to_le_bytes());
    } else {
        b[4..8].copy_from_slice(&(ifd as u32).to_le_bytes());
    }
    let mut tags = vec![
        [256, 4, 1, 1],
        [257, 4, 1, 1],
        [258, 3, 1, 8],
        [262, 3, 1, 1],
        [273, 4, 1, data],
        [278, 4, 1, 1],
        [279, 4, 1, 1],
    ];
    tags.extend_from_slice(extra);
    put(&mut b, tags.len() as u64, if big { 8 } else { 2 });
    let w = if big { 8 } else { 4 };
    for t in tags {
        put(&mut b, t[0], 2);
        put(&mut b, t[1], 2);
        put(&mut b, t[2], w);
        put(&mut b, t[3], w);
    }
    put(&mut b, 0, w);
    b
}
