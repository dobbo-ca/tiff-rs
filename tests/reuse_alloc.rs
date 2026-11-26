//! Buffer reuse (docs/performance.md): a `Decoder` that has decoded a chunk once decodes
//! it again, at `max_threads` 1, without one call to the allocator, for every codec class below;
//! and a Deflate chunk that is smaller (in compressed and in decoded bytes) than the one it
//! decoded before.
//! This binary counts every allocation (`alloc`, `alloc_zeroed`, `realloc`), so it holds exactly
//! one test: another test running beside it would be counted too.
//!
//! Exempt, and listed in docs/performance.md: a progressive or non-interleaved JPEG frame (its
//! `jpeg::RowReader` builds its coefficient buffers on every call; jpeg-rs puts `RowReader` out of
//! its reuse scope), a four-component JPEG (`codec/jpeg4.rs` reads its chunk into new buffers), and
//! a Deflate or JPEG chunk that does not fit the ledger or the band limit, which streams. They are
//! not measured here. CCITT Group 3 has no fixture that this crate reads (the 2-D and fill-bit
//! files are refused), so only Group 4 is.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use tiff::{Decoder, File, Limits};

/// Counts the calls that ask the allocator for memory.
struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

/// How many allocator calls of a measured window `LOG` keeps.
const LOG_LEN: usize = 16;
/// `ALLOCS` when the window opened.
static BASE: AtomicUsize = AtomicUsize::new(0);
/// The first `LOG_LEN` calls of the window as `(size << 2) | kind`; atomics, so the allocator
/// itself never allocates to record them.
static LOG: [AtomicUsize; LOG_LEN] = [const { AtomicUsize::new(0) }; LOG_LEN];
const KINDS: [&str; 3] = ["alloc", "alloc_zeroed", "realloc"];

/// Counts one allocator call of `size` bytes (`kind` indexes `KINDS`) and files it in `LOG`.
fn note(size: usize, kind: usize) {
    let k = ALLOCS
        .fetch_add(1, Relaxed)
        .wrapping_sub(BASE.load(Relaxed));
    if k < LOG_LEN {
        LOG[k].store((size << 2) | kind, Relaxed);
    }
}

/// Opens a measured window and returns the count at its start.
fn open_window() -> usize {
    let now = ALLOCS.load(Relaxed);
    BASE.store(now, Relaxed);
    now
}

/// The first allocator calls of the window that just closed with `n` of them, by kind and size.
fn first_allocs(n: usize) -> String {
    (0..n.min(LOG_LEN))
        .map(|k| {
            let v = LOG[k].load(Relaxed);
            format!("{} {}", KINDS[v & 3], v >> 2)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// SAFETY: every method forwards to `System` with the caller's arguments, so System's contract
// carries over; the counter is an atomic.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        note(l.size(), 0);
        // SAFETY: the caller upholds `alloc`'s contract, which `System.alloc` has too.
        unsafe { System.alloc(l) }
    }

    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        note(l.size(), 1);
        // SAFETY: as for `alloc`.
        unsafe { System.alloc_zeroed(l) }
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: `p` came from this allocator with layout `l`, so from System with it.
        unsafe { System.dealloc(p, l) };
    }

    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        note(new, 2);
        // SAFETY: `p` came from this allocator with layout `l`, and `new` is valid for it.
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// One fixture per codec class: its name and its file in `tests/data/golden`.
const CASES: &[(&str, &str)] = &[
    ("none strip", "grey_none_strip.tif"),
    ("none tile", "grey_none_tile.tif"),
    ("packbits strip", "grey_packbits_strip.tif"),
    ("packbits tile 16-bit", "g16_packbits_tile.tif"),
    ("lzw strip", "grey_lzw_strip.tif"),
    ("lzw tile with predictor", "grey_lzwp2_tile.tif"),
    ("deflate strip", "grey_zip_strip.tif"),
    ("deflate tile", "rgb_zip_tile.tif"),
    ("deflate strip with predictor", "grey_zip-p2_strip.tif"),
    ("deflate 16-bit with predictor", "g16_zip-p2_tile.tif"),
    ("jpeg grey strip", "grey_jpeg_strip.tif"),
    ("jpeg baseline strip", "rgb_jpeg_strip.tif"),
    ("jpeg baseline tile", "rgb_jpeg_tile.tif"),
    ("ccitt group 4", "bw_g4_r16.tif"),
];

#[test]
fn second_decode_allocates_nothing() {
    // One thread: a call that threads allocates job lists, whatever the decoder keeps.
    let lim = Limits {
        max_threads: 1,
        ..Limits::default()
    };
    let cancel = AtomicBool::new(false);
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/golden");
    for (name, file) in CASES {
        let b = std::fs::read(dir.join(file)).unwrap();
        let f = File::open(b.as_slice(), b.len() as u64, &lim).unwrap();
        let page = f.page(0).unwrap();
        let mut dec = Decoder::new();
        let mut out = Vec::new();
        // The first call builds the tables and sizes the buffers.
        page.decode_chunk_into(0, &mut dec, &mut out, &cancel)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let want = out.clone();
        assert!(!want.is_empty(), "{name}");

        let before = open_window();
        let r = page.decode_chunk_into(0, &mut dec, &mut out, &cancel);
        let calls = ALLOCS.load(Relaxed) - before;
        r.unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(out, want, "{name}: the second decode differs");
        assert_eq!(
            calls,
            0,
            "{name}: {calls} allocator calls in the second decode: {}",
            first_allocs(calls)
        );
    }
    smaller_after_larger(&lim, &cancel);
}

/// A one-chunk RGB page of `side` pixels square.
fn square(compression: usize, side: usize) -> Vec<u8> {
    common::File {
        pages: vec![common::Page {
            w: side,
            h: side,
            spp: 3,
            bps: 8,
            photometric: 2,
            compression,
            pix: Some(common::pattern(side, side, 3, 8)),
            ..Default::default()
        }],
        ..Default::default()
    }
    .bytes()
}

/// A `Decoder` warmed on a larger chunk decodes a smaller one without a call to the allocator. It
/// runs inside the one test, so that nothing else is counted. Only Deflate is measured: its
/// buffers are tiff-rs's own, kept by capacity. jpeg-rs promises no allocation only for "a frame
/// of the same shape" (its `Decoder` docs); for a smaller frame it picks pooled buffers by size
/// and may allocate, so a JPEG case here could fail for a reason that is not a defect. The JPEG
/// buffers tiff-rs keeps (`src`, `jrows`) are covered by the same-chunk case above.
fn smaller_after_larger(lim: &Limits, cancel: &AtomicBool) {
    let (name, compression) = ("deflate", 8);
    let (big, small) = (square(compression, 256), square(compression, 64));
    let fb = File::open(big.as_slice(), big.len() as u64, lim).unwrap();
    let fs = File::open(small.as_slice(), small.len() as u64, lim).unwrap();
    let (pb, ps) = (fb.page(0).unwrap(), fs.page(0).unwrap());
    let mut dec = Decoder::new();
    let mut out = Vec::new();
    pb.decode_chunk_into(0, &mut dec, &mut out, cancel)
        .unwrap_or_else(|e| panic!("{name}: {e}"));

    let before = open_window();
    let r = ps.decode_chunk_into(0, &mut dec, &mut out, cancel);
    let calls = ALLOCS.load(Relaxed) - before;
    r.unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(out.len(), 64 * 64 * 3, "{name}");
    assert_eq!(
        calls,
        0,
        "{name}: {calls} allocator calls for a smaller chunk: {}",
        first_allocs(calls)
    );
}
