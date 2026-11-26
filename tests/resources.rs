//! Resource controls (docs/limits.md, docs/threading.md): for every `max_threads` the bands and the
//! error are those of `max_threads` 1, and one call never books more than `max_alloc` or runs on
//! more than `max_threads` threads. The pool tests are in `pool.rs`, which holds process state.

mod common;

use common::{ALLOCS, File, Page, collect, fixtures, pattern};
use std::fs;
use std::io;
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;
use tiff::{Error, Limits, ReadAt};

fn thread_counts() -> Vec<usize> {
    let mut t = vec![1, 2, 4, Limits::default().max_threads];
    t.dedup();
    t
}

// (a) the bands, (b) the errors and (c) the peaks, for every thread count and several limits.
#[test]
fn bands_errors_and_peaks_match_serial() {
    for (name, b) in fixtures() {
        for band in [Limits::default().max_band_bytes, 4096] {
            for max_alloc in ALLOCS {
                let base = Limits {
                    max_band_bytes: band,
                    max_alloc,
                    max_threads: 1,
                    ..Default::default()
                };
                let (want, want_err, p1) = collect(b.as_slice(), b.len() as u64, &base, None);
                assert_eq!(p1.threads_peak, 1, "{name}: serial threads");
                assert!(p1.charged_peak <= max_alloc, "{name}: serial peak");
                for t in thread_counts() {
                    let lim = Limits {
                        max_threads: t,
                        ..base
                    };
                    let (got, err, probe) = collect(b.as_slice(), b.len() as u64, &lim, None);
                    let at = format!("{name} band {band} alloc {max_alloc} threads {t}");
                    assert!(got == want, "{at}: bands differ");
                    assert_eq!(err, want_err, "{at}: error differs");
                    assert!(probe.charged_peak <= max_alloc, "{at}: charged {probe:?}");
                    assert!(probe.threads_peak <= t.max(1), "{at}: threads {probe:?}");
                    if t <= 1 {
                        assert_eq!(probe.threads_peak, 1, "{at}");
                    }
                }
            }
        }
    }
}

// max_threads 0 counts as 1.
#[test]
fn zero_threads_is_one() {
    let (_, b) = fixtures().remove(0);
    let lim = Limits {
        max_threads: 0,
        ..Default::default()
    };
    let (bands, err, probe) = collect(b.as_slice(), b.len() as u64, &lim, None);
    assert_eq!((err, probe.threads_peak), (None, 1));
    assert!(!bands.is_empty());
}

/// The bands before a Break or a cancel set by the callback, and the error.
fn stopped(
    b: &[u8],
    lim: &Limits,
    after: usize,
    cancel_it: bool,
) -> (Vec<(usize, Vec<u8>)>, Option<Error>) {
    let f = tiff::File::open(b, b.len() as u64, lim).unwrap();
    let p = f.page(0).unwrap();
    let flag = AtomicBool::new(false);
    let mut bands = Vec::new();
    let r = p.rows(&flag, |y, rows| {
        bands.push((y, rows.to_vec()));
        if bands.len() >= after {
            if cancel_it {
                flag.store(true, Ordering::Relaxed);
            } else {
                return ControlFlow::Break(());
            }
        }
        ControlFlow::Continue(())
    });
    (bands, r.err())
}

// A Break or a cancel from the callback stops all workers, at the same band as serially.
#[test]
fn break_and_cancel_match_serial() {
    for (name, b) in fixtures() {
        for cancel_it in [false, true] {
            for after in [1, 3] {
                let want = stopped(
                    &b,
                    &Limits {
                        max_threads: 1,
                        ..Default::default()
                    },
                    after,
                    cancel_it,
                );
                for t in [2, 4] {
                    let lim = Limits {
                        max_threads: t,
                        ..Default::default()
                    };
                    let got = stopped(&b, &lim, after, cancel_it);
                    assert!(
                        got == want,
                        "{name} after {after} cancel {cancel_it} threads {t}"
                    );
                }
            }
        }
    }
}

// The memory for the workers is booked: a limit that holds one chunk or two still gives the
// serial bands, and a call with room uses more than one thread on a page of many chunks.
#[test]
fn workers_fit_in_the_ledger() {
    let (w, h) = (256, 256);
    let p = Page {
        w,
        h,
        spp: 3,
        bps: 8,
        photometric: 6, // jpeg-rs writes YCbCr
        compression: 7,
        rows_per_strip: 4,
        pix: Some(pattern(w, h, 3, 8)),
        ..Default::default()
    };
    let b = File {
        pages: vec![p],
        ..Default::default()
    }
    .bytes();
    let mut most = 1;
    for max_alloc in [1 << 30, 6 << 20, 2 << 20] {
        // A small band keeps the serial JPEG budget (four bands and a megabyte) within reach.
        let serial = Limits {
            max_threads: 1,
            max_band_bytes: 1 << 16,
            max_alloc,
            ..Default::default()
        };
        let (want, want_err, _) = collect(b.as_slice(), b.len() as u64, &serial, None);
        assert_eq!(want_err, None, "alloc {max_alloc}");
        let lim = Limits {
            max_threads: 4,
            ..serial
        };
        // A pool task that wakes after the caller took every chunk is not a worker the call
        // used: take the most of a few tries.
        for _ in 0..30 {
            let (got, err, probe) = collect(b.as_slice(), b.len() as u64, &lim, None);
            assert!(got == want && err.is_none(), "alloc {max_alloc}");
            assert!(probe.charged_peak <= max_alloc && probe.threads_peak <= 4);
            if max_alloc == 1 << 30 {
                most = most.max(probe.threads_peak);
            }
            if most > 1 {
                break;
            }
        }
    }
    if std::thread::available_parallelism().map_or(1, |n| n.get()) > 1 {
        assert!(most > 1, "no call used a second thread");
    }
}

/// Returns at most `k` bytes per read.
struct Short<'a>(&'a [u8], usize);

impl ReadAt for Short<'_> {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        let n = buf.len().min(self.1);
        self.0.read_at(&mut buf[..n], offset)
    }
}

/// Fails every read.
struct Failing;

impl ReadAt for Failing {
    fn read_at(&self, _: &mut [u8], _: u64) -> io::Result<usize> {
        Err(io::Error::other("injected"))
    }
}

// The fuzz target `readat` found this input to run past its timeout (a strip of Deflate with
// predictor 2 at 16 bits, one strip with a bad zlib header). A source that returns one byte per
// read, one that always fails, and every thread count must finish within 10 s with the result
// of a plain slice.
#[test]
fn readat_timeout_input() {
    let b =
        fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/fuzz/readat-timeout.tif"))
            .unwrap();
    let (tx, rx) = mpsc::channel();
    // On its own thread, so a loop that cannot make progress fails the test instead of hanging it.
    std::thread::spawn(move || {
        let size = b.len() as u64;
        for t in [1, 4] {
            let lim = Limits {
                max_ifds: 64,
                max_pixels: 1 << 22,
                max_band_bytes: 1 << 20,
                max_metadata_bytes: 1 << 16,
                max_alloc: 64 << 20,
                max_threads: t,
            };
            let want = collect(b.as_slice(), size, &lim, None);
            for k in [1, 2, 7, 16] {
                let got = collect(Short(&b, k), size, &lim, None);
                assert!(
                    got.0 == want.0 && got.1 == want.1,
                    "short reads of {k} change the result"
                );
            }
            let failed = collect(Failing, size, &lim, None);
            assert!(matches!(failed.1, Some(Error::Io(_))), "{:?}", failed.1);
        }
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("the input did not finish within 10 s");
}
