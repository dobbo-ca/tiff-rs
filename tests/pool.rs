//! Which threads a call runs on (docs/threading.md): `max_threads` 1 never starts the crate's
//! shared pool, a caller's pool is used instead of it, and a call worth threading starts it. The
//! shared pool is process state, so this binary holds one test and runs the steps in order.

mod common;

use common::{File, Page, collect, pattern};
use tiff::{Limits, ThreadPool, shared_pool_started};

fn limits(max_threads: usize) -> Limits {
    Limits {
        max_threads,
        ..Default::default()
    }
}

#[test]
fn pools() {
    // Many strips of work that costs something.
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
    let size = b.len() as u64;
    // The most threads of a call in up to 30 tries: a pool task that wakes after the caller took
    // every chunk is not a worker the call used.
    let peak = |t: usize, pool: Option<&ThreadPool>| {
        let mut best = 1;
        for _ in 0..30 {
            best = best.max(collect(b.as_slice(), size, &limits(t), pool).2.threads_peak);
            if best > 1 {
                break;
            }
        }
        best
    };
    let (want, err, _) = collect(b.as_slice(), size, &limits(1), None);
    assert_eq!(err, None);

    // max_threads 1 never starts the shared pool.
    assert!(!shared_pool_started());
    assert_eq!(peak(1, None), 1);
    assert!(!shared_pool_started());

    // A caller's pool is used, and the shared one is not started.
    let pool = ThreadPool::new(3).unwrap();
    assert_eq!(pool.threads(), 3);
    let (got, err, _) = collect(b.as_slice(), size, &limits(4), Some(&pool));
    assert!(got == want && err.is_none());
    assert!(peak(4, Some(&pool)) > 1, "the caller's pool was not used");
    assert!(!shared_pool_started(), "a caller's pool must win");

    // A pool of 1 thread gives a call two threads at most: itself and that one.
    let one = ThreadPool::new(1).unwrap();
    assert!(peak(8, Some(&one)) <= 2);
    assert!(!shared_pool_started());

    // Without a pool, a call worth threading starts the shared one and is capped by max_threads.
    let (got, err, probe) = collect(b.as_slice(), size, &limits(4), None);
    assert!(got == want && err.is_none() && probe.threads_peak <= 4);
    assert!(shared_pool_started());
}
