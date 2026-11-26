//! The real heap against `max_alloc`: the ledger books what the decoder asks for, so its peak
//! cannot pass `max_alloc` whatever the code allocates outside it. This binary counts every
//! allocation of the process (a global allocator, live bytes and their peak) and asserts that the
//! live bytes `rows` adds stay within `max_alloc` and a small fixed allowance, and that those
//! `rows_probe` adds stay within the bytes the call booked (`charged_peak`) and the same allowance,
//! over the matrix of `resources.rs`, a large JPEG strip and the same JPEG in strips, and a tiled
//! JPEG page whose chunks differ in size (small first, large later) at 2 and 4 threads. The same
//! bound is asserted for `Page::decode_chunk_into` over every codec fixture with a cold `Decoder`,
//! one warmed on the same chunk and one warmed on larger ones (the memory the `Decoder` keeps
//! counts), and for `Page::raw_jpeg_into`. It holds one test, so that nothing else allocates while
//! a call is measured.

mod common;

use common::{ALLOCS, File, Page, big_jpeg, big_jpeg_strips, collect, fixtures, pattern};
use std::alloc::{GlobalAlloc, Layout, System};
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use tiff::{Decoder, Limits};

/// Bytes allocated and not freed, and the most of them at once since the last reset.
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct Counting;

fn grow(n: usize) {
    let live = LIVE.fetch_add(n, Relaxed) + n;
    PEAK.fetch_max(live, Relaxed);
}

// SAFETY: every method forwards its arguments unchanged to `System`, which upholds the contract
// of `GlobalAlloc`; the counters are atomics and never touch the memory.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        // SAFETY: `l` is the caller's layout, as `GlobalAlloc::alloc` requires.
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            grow(l.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        // SAFETY: `l` is the caller's layout, as `GlobalAlloc::alloc_zeroed` requires.
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() {
            grow(l.size());
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: `p` and `l` are those of a live block of this allocator, forwarded unchanged.
        unsafe { System.dealloc(p, l) };
        LIVE.fetch_sub(l.size(), Relaxed);
    }

    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        // SAFETY: `p`, `l` and `new` are the caller's, forwarded unchanged.
        let q = unsafe { System.realloc(p, l, new) };
        if !q.is_null() {
            if new >= l.size() {
                grow(new - l.size());
            } else {
                LIVE.fetch_sub(l.size() - new, Relaxed);
            }
        }
        q
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// What the heap may hold beyond `max_alloc`, because the ledger does not book it: the codecs'
/// fixed tables (LZW about 16 KiB, the CCITT and JPEG tables), a reader's 4 KiB buffer, the boxed
/// readers, rayon's job bookkeeping per task and the registry of a pool started during the call,
/// for up to 4 threads. The sum of these is some hundreds of KiB (the pools are started before
/// anything is measured, and the test prints the largest `got - charged_peak` it saw, so a CI log
/// shows the margin); 1 MiB leaves room for them. A buffer that scales with the input (a band
/// copy or a decoded plane of the large images, 1.5 MiB and more) is over it.
const ALLOWANCE: u64 = 1 << 20;

/// The most live bytes `rows` of page 0 added over the heap just after `page` returned, the most
/// `rows_probe` added, and the most that call booked; all 0 if the file does not open or parse
/// under `lim` (a limit refused it before any call).
fn peak(b: &[u8], lim: &Limits) -> (u64, u64, u64) {
    let Ok(f) = tiff::File::open(b, b.len() as u64, lim) else {
        return (0, 0, 0);
    };
    let Ok(page) = f.page(0) else {
        return (0, 0, 0);
    };
    let cancel = AtomicBool::new(false);
    let base = LIVE.load(Relaxed);
    PEAK.store(base, Relaxed);
    // The bands are dropped, not collected: the test's own copies are not the decoder's.
    let _ = page.rows(&cancel, |_, _| ControlFlow::Continue(()));
    let rows = PEAK.load(Relaxed).saturating_sub(base) as u64;
    let base = LIVE.load(Relaxed);
    PEAK.store(base, Relaxed);
    let (_, probe) = page.rows_probe(&cancel, |_, _| ControlFlow::Continue(()));
    let probed = PEAK.load(Relaxed).saturating_sub(base) as u64;
    (rows, probed, probe.charged_peak)
}

/// A tiled JPEG page of 4 by 2 tiles of 64 by 64 pixels. The first four chunks are small; the
/// others have about `pad` bytes of comment segments in front of the same image.
fn varied_tiles(pad: usize) -> Vec<u8> {
    let mut pix = vec![255u8; 4 * 64 * 64];
    for (i, p) in pix.iter_mut().enumerate() {
        if i % 4 != 3 {
            *p = (i * 7 % 251) as u8;
        }
    }
    let m = jpeg::Rgba {
        pix,
        stride: 4 * 64,
        rect: jpeg::Rect::new(0, 0, 64, 64),
    };
    let o = jpeg::EncodeOptions {
        quality: 50,
        chroma444: false,
    };
    let tile = jpeg::encode(
        jpeg::EncodeImage::Rgba(&m),
        &o,
        &jpeg::Limits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let mut padded = tile[..2].to_vec();
    for _ in 0..pad / 65537 {
        padded.extend_from_slice(&[0xff, 0xfe, 0xff, 0xff]);
        padded.extend(std::iter::repeat_n(0u8, 65533));
    }
    padded.extend_from_slice(&tile[2..]);
    let chunks = (0..8)
        .map(|i| if i < 4 { tile.clone() } else { padded.clone() })
        .collect();
    File {
        pages: vec![Page {
            w: 256,
            h: 128,
            spp: 3,
            bps: 8,
            photometric: 6, // jpeg-rs writes YCbCr (4:2:0)
            compression: 7,
            tile_w: 64,
            tile_l: 64,
            chunks: Some(chunks),
            ..Default::default()
        }],
        ..Default::default()
    }
    .bytes()
}

/// One 1024 by 512 RGB strip of Deflate (stored blocks): a chunk of 1.5 MiB in and out.
fn big_deflate() -> Vec<u8> {
    File {
        pages: vec![Page {
            w: 1024,
            h: 512,
            spp: 3,
            bps: 8,
            photometric: 2,
            compression: 8,
            pix: Some(pattern(1024, 512, 3, 8)),
            ..Default::default()
        }],
        ..Default::default()
    }
    .bytes()
}

/// Decodes chunk `index` (the last one if it is past the end) of page 0 of `b` through a
/// `Decoder` that first decoded chunk 0 of each file of `warm` under the default limits. Returns
/// the most live bytes during the call over the heap before the `Decoder` was warmed, the most
/// over the heap when the call began, and the bytes the `Decoder` kept from the warming; `None`
/// if the file does not open or parse under `lim`.
fn chunk_peak(b: &[u8], lim: &Limits, warm: &[&[u8]], index: usize) -> Option<(u64, u64, u64)> {
    let f = tiff::File::open(b, b.len() as u64, lim).ok()?;
    let page = f.page(0).ok()?;
    let index = index.min(page.layout().chunks.saturating_sub(1));
    let cancel = AtomicBool::new(false);
    let base = LIVE.load(Relaxed);
    let mut dec = Decoder::new();
    {
        let one = Limits {
            max_threads: 1,
            ..Default::default()
        };
        let mut scratch = Vec::new();
        for w in warm {
            if let Ok(wf) = tiff::File::open(*w, w.len() as u64, &one)
                && let Ok(wp) = wf.page(0)
            {
                let _ = wp.decode_chunk_into(0, &mut dec, &mut scratch, &cancel);
            }
        }
    }
    let start = LIVE.load(Relaxed);
    let mut out = Vec::new();
    PEAK.store(start, Relaxed);
    let _ = page.decode_chunk_into(index, &mut dec, &mut out, &cancel);
    let peak = PEAK.load(Relaxed);
    Some((
        peak.saturating_sub(base) as u64,
        peak.saturating_sub(start) as u64,
        start.saturating_sub(base) as u64,
    ))
}

/// Whether the warmed-on-larger variant runs for this `band` and `max_alloc`.
fn warm_on_larger(band: u64, max_alloc: u64) -> bool {
    band == Limits::default().max_band_bytes && (max_alloc == ALLOCS[1] || max_alloc == ALLOCS[2])
}

/// `decode_chunk_into` over every fixture, cold, warmed on the same chunk and warmed on larger
/// chunks of JPEG and Deflate, for each `max_alloc`: the heap peak, the `Decoder`'s own memory
/// included, stays within `max_alloc` and the allowance. A `Decoder` that keeps more than
/// `max_alloc` frees it at the start of the call, so then only what the call adds is bounded.
fn chunk_cases(small: &[(String, Vec<u8>)]) {
    let (big, deflate) = (big_jpeg(), big_deflate());
    for (name, b) in small {
        for band in [Limits::default().max_band_bytes, 4096] {
            for max_alloc in ALLOCS {
                let lim = Limits {
                    max_band_bytes: band,
                    max_alloc,
                    max_threads: 4,
                    ..Default::default()
                };
                let warms: [(&str, Vec<&[u8]>); 3] = [
                    ("cold", vec![]),
                    ("warm", vec![b.as_slice()]),
                    ("warmed on larger", vec![big.as_slice(), deflate.as_slice()]),
                ];
                // Warming on the large JPEG is the slow part: two `max_alloc` values (the large
                // chunks kept, and over the limit so freed), the first chunk, the default band.
                let larger = warm_on_larger(band, max_alloc);
                for index in [0, usize::MAX] {
                    let n = if larger && index == 0 { 3 } else { 2 };
                    for (kind, warm) in &warms[..n] {
                        let Some((total, growth, kept)) = chunk_peak(b, &lim, warm, index) else {
                            continue;
                        };
                        let at = format!(
                            "{name} chunk {index} band {band} alloc {max_alloc} {kind}: \
                             the Decoder kept {kept} bytes"
                        );
                        if kept <= max_alloc {
                            assert!(
                                total <= max_alloc + ALLOWANCE,
                                "{at}: the heap peaked {total} bytes"
                            );
                        } else {
                            assert!(
                                growth <= max_alloc + ALLOWANCE,
                                "{at}: the call added {growth} bytes"
                            );
                        }
                    }
                }
            }
        }
    }
}

/// The most live bytes `raw_jpeg_into` of chunk `index` of page 0 of `b` adds; `None` if the file
/// does not open or parse under `lim`.
fn raw_peak(b: &[u8], lim: &Limits, index: usize) -> Option<u64> {
    let f = tiff::File::open(b, b.len() as u64, lim).ok()?;
    let page = f.page(0).ok()?;
    let index = index.min(page.layout().chunks.saturating_sub(1));
    let mut out = Vec::new();
    let start = LIVE.load(Relaxed);
    PEAK.store(start, Relaxed);
    let _ = page.raw_jpeg_into(index, &mut out);
    Some(PEAK.load(Relaxed).saturating_sub(start) as u64)
}

#[test]
fn heap_peak_stays_within_max_alloc() {
    // Starts the pools (thread bookkeeping) and warms the code paths before anything is measured.
    let big = big_jpeg();
    let lim = Limits {
        max_threads: 4,
        ..Default::default()
    };
    let strips = big_jpeg_strips();
    let _ = collect(big.as_slice(), big.len() as u64, &lim, None);
    let _ = collect(strips.as_slice(), strips.len() as u64, &lim, None);
    let small = fixtures();
    let _ = collect(small[0].1.as_slice(), small[0].1.len() as u64, &lim, None);

    let mut cases: Vec<(String, &[u8], u64, u64)> = Vec::new();
    for (name, b) in &small {
        for band in [Limits::default().max_band_bytes, 4096] {
            for max_alloc in ALLOCS {
                cases.push((name.clone(), b.as_slice(), band, max_alloc));
            }
        }
    }
    // The large strip: room for the call (about 11 MiB at a 2 MiB band) and not enough. The same
    // image in 8 strips runs the pipeline's workers (max_threads 2 and 4) on chunks of 192 KiB.
    for (name, b) in [("big jpeg strip", &big), ("big jpeg strips", &strips)] {
        for band in [1 << 20, 2 << 20] {
            for max_alloc in [16 << 20, 12 << 20, 8 << 20, 6 << 20] {
                cases.push((name.into(), b.as_slice(), band, max_alloc));
            }
        }
    }
    let mut margin = 0;
    for (name, b, max_band_bytes, max_alloc) in cases {
        for max_threads in [1, 2, 4] {
            let lim = Limits {
                max_band_bytes,
                max_alloc,
                max_threads,
                ..Default::default()
            };
            let (got, probed, charged) = peak(b, &lim);
            assert!(
                got <= max_alloc + ALLOWANCE,
                "{name} band {max_band_bytes} alloc {max_alloc} threads {max_threads}: \
                 the heap peaked {got} bytes over its start"
            );
            assert!(
                probed <= charged + ALLOWANCE,
                "{name} band {max_band_bytes} alloc {max_alloc} threads {max_threads}: \
                 the heap peaked {probed} bytes over its start, the call booked {charged}"
            );
            margin = margin.max(probed.saturating_sub(charged));
        }
    }
    println!("the largest heap peak over what was booked: {margin} bytes (allowance {ALLOWANCE})");

    // Chunks that differ in size: the buffers a worker keeps from a large chunk are booked when it
    // decodes the next, so the heap stays within `max_alloc` at 2 and 4 threads. The large chunks
    // are padded to `max_alloc` less the budget of a JPEG call (1.06 MiB at this band) and 1 MiB:
    // one copy of such a chunk that the ledger does not book is over the allowance by itself.
    let mut tiles = Vec::new();
    for max_alloc in [8 << 20, 5 << 20] {
        let t = varied_tiles(max_alloc as usize - (1 << 20) - (1 << 20));
        for max_threads in [2, 4] {
            let lim = Limits {
                max_band_bytes: 1 << 16,
                max_alloc,
                max_threads,
                ..Default::default()
            };
            let (got, probed, charged) = peak(&t, &lim);
            let at = format!("varied tiles alloc {max_alloc} threads {max_threads}");
            // The case measures nothing if the large tiles fail early: they must decode.
            let f = tiff::File::open(t.as_slice(), t.len() as u64, &lim).unwrap();
            f.page(0)
                .unwrap()
                .rows(&AtomicBool::new(false), |_, _| ControlFlow::Continue(()))
                .unwrap_or_else(|e| panic!("{at}: {e}"));
            assert!(got <= max_alloc + ALLOWANCE, "{at}: the heap peaked {got}");
            assert!(
                probed <= charged + ALLOWANCE,
                "{at}: the heap peaked {probed}, the call booked {charged}"
            );
        }
        tiles = t;
    }

    chunk_cases(&small);

    // `raw_jpeg_into` allocates the stream once, after checking it against `max_alloc`.
    for (name, b) in [("big jpeg", &big), ("varied tiles", &tiles)] {
        for max_alloc in ALLOCS.into_iter().chain([3 << 20, 8 << 20]) {
            let lim = Limits {
                max_alloc,
                ..Default::default()
            };
            for index in [0, usize::MAX] {
                if let Some(got) = raw_peak(b, &lim, index) {
                    assert!(
                        got <= max_alloc + ALLOWANCE,
                        "{name} chunk {index} alloc {max_alloc}: raw_jpeg_into added {got} bytes"
                    );
                }
            }
        }
    }
}
