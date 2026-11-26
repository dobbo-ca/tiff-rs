//! Nested jpeg-rs threads (docs/threading.md, docs/limits.md): a call that runs serially on one
//! large JPEG strip gives jpeg-rs the caller's pool, never starts jpeg-rs's shared pool when it
//! has one, and its thread peak counts jpeg-rs's threads. `Page::decode_chunk_into` at
//! `max_threads` 4 spawns no thread and starts neither pool. Both shared pools are process state,
//! so this binary holds one test and runs the steps in order.

mod common;

use common::{big_jpeg, collect};
use std::ops::ControlFlow;
use std::sync::atomic::AtomicBool;
use tiff::{Limits, ThreadPool, shared_pool_started};

/// The threads of this process where `/proc` lists them (Linux), `None` elsewhere.
fn os_threads() -> Option<usize> {
    Some(std::fs::read_dir("/proc/self/task").ok()?.count())
}

fn limits(max_threads: usize) -> Limits {
    Limits {
        max_threads,
        ..Default::default()
    }
}

#[test]
fn nested_jpeg_threads() {
    // One 1024x512 strip is one band at the default `max_band_bytes`: jpeg-rs's colour conversion
    // (one worker per 131072 pixels of the decoded region) gets 4 workers, so 2 at max_threads 2.
    let b = big_jpeg();
    let size = b.len() as u64;
    let (want, err, p1) = collect(b.as_slice(), size, &limits(1), None);
    assert_eq!(err, None);
    assert_eq!(p1.threads_peak, 1);
    // max_threads 1 starts no pool at all.
    assert!(!shared_pool_started() && !jpeg::shared_pool_started());

    // One chunk on the calling thread at max_threads 4: no thread, no pool, the same pixels.
    {
        let f = tiff::File::open(b.as_slice(), size, &limits(4)).unwrap();
        let page = f.page(0).unwrap();
        let (mut dec, mut chunk) = (tiff::Decoder::new(), Vec::new());
        let before = os_threads();
        page.decode_chunk_into(0, &mut dec, &mut chunk, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(os_threads(), before, "decode_chunk_into spawned a thread");
        assert!(chunk == want[0].1);
        assert!(
            !shared_pool_started() && !jpeg::shared_pool_started(),
            "decode_chunk_into started a pool"
        );
    }

    // The most threads of a call in up to 30 tries, as `pool.rs` does: a pool thread that wakes
    // after the caller did the work is not a worker the call used. Every try matches `want`.
    let peak = |t: usize, pool: Option<&ThreadPool>| {
        let mut best = 1;
        for _ in 0..30 {
            let (got, err, probe) = collect(b.as_slice(), size, &limits(t), pool);
            assert!(got == want && err.is_none(), "threads {t}");
            assert!(probe.threads_peak <= t, "threads {t}: {probe:?}");
            best = best.max(probe.threads_peak);
            if best > 1 {
                break;
            }
        }
        best
    };

    // A caller's pool serves jpeg-rs, through the probe and through `rows`: its own shared pool is
    // not started, and the peak counts jpeg-rs's threads.
    let pool = ThreadPool::new(2).unwrap();
    for t in [2, 4] {
        assert!(
            peak(t, Some(&pool)) > 1,
            "threads {t}: jpeg-rs did not thread"
        );
    }
    let f = tiff::File::open(b.as_slice(), size, &limits(4))
        .unwrap()
        .with_pool(pool.clone());
    let mut got = Vec::new();
    f.page(0)
        .unwrap()
        .rows(&AtomicBool::new(false), |y, band| {
            got.push((y, band.to_vec()));
            ControlFlow::Continue(())
        })
        .unwrap();
    assert!(got == want);
    assert!(
        !jpeg::shared_pool_started() && !shared_pool_started(),
        "a caller's pool must win over jpeg-rs's shared pool"
    );

    // Without a pool: at most max_threads, 1 at 1, and jpeg-rs's threads counted above 1.
    assert_eq!(peak(1, None), 1);
    for t in [2, 4] {
        assert!(peak(t, None) > 1, "threads {t}: jpeg-rs did not thread");
    }
}
