#![no_main]
use libfuzzer_sys::fuzz_target;
use std::sync::LazyLock;
use std::sync::atomic::AtomicBool;
use tiff::{Error, Limits, ThreadPool};

#[path = "../common.rs"]
mod common;
use common::{MAX_PAGES, Out, TIGHT, budget, rows_probe_with, run_file};

/// A caller's pool of 4 threads, for the pass that runs on it.
static POOL: LazyLock<ThreadPool> = LazyLock::new(|| ThreadPool::new(4).unwrap());

/// Every page of `data` decoded with `threads` (on `pool` if given), stopped with `Break` after
/// `stop` bands and cancelled from the callback after `cancel_after`; asserts the call's peaks.
fn outs(
    data: &[u8],
    threads: usize,
    stop: usize,
    cancel_after: usize,
    pool: Option<&ThreadPool>,
) -> Vec<Out> {
    let lim = Limits {
        max_threads: threads,
        ..TIGHT
    };
    let Ok(f) = tiff::File::open(data, data.len() as u64, &lim) else {
        return Vec::new();
    };
    let f = match pool {
        Some(p) => f.with_pool(p.clone()),
        None => f,
    };
    let mut v = Vec::new();
    for i in 0..f.num_pages().min(MAX_PAGES) {
        let Ok(p) = f.page(i) else { continue };
        let cancel = AtomicBool::new(false);
        let (o, probe) = rows_probe_with(&p, &lim, &cancel, stop, cancel_after);
        assert!(
            probe.threads_peak <= threads,
            "more threads than max_threads"
        );
        assert!(
            probe.charged_peak <= lim.max_alloc,
            "charged over max_alloc"
        );
        v.push(o);
    }
    v
}

// Arbitrary bytes as a TIFF, decoded with one thread and with four: the band sequence (every
// band's y and bytes), the page info and the error of every page must be identical. Then the
// same with a `Break` after b bands (identical again), a cancel after c bands (b and c from the
// last two bytes): the four-thread result is a prefix of the full one, ending in Cancelled unless
// the flag came after the last check, and four threads on a caller's pool (identical to the full
// one-thread result). Each of these seven decode passes is budgeted separately, which fuzz.yml's
// -timeout for this target allows for.
fuzz_target!(|data: &[u8]| {
    let size = data.len() as u64;
    let one = run_file(
        data,
        size,
        &Limits {
            max_threads: 1,
            ..TIGHT
        },
    );
    let four = run_file(
        data,
        size,
        &Limits {
            max_threads: 4,
            ..TIGHT
        },
    );
    assert!(one == four, "max_threads changes the result");
    let &[b, c] = data.last_chunk::<2>().unwrap_or(&[0, 0]);
    let (b, c) = (usize::from(b % 8) + 1, usize::from(c % 8) + 1);
    let stopped1 = budget(|| outs(data, 1, b, usize::MAX, None));
    let stopped4 = budget(|| outs(data, 4, b, usize::MAX, None));
    assert!(stopped1 == stopped4, "max_threads changes a stopped decode");
    let full = budget(|| outs(data, 1, usize::MAX, usize::MAX, None));
    let cancelled = budget(|| outs(data, 4, usize::MAX, c, None));
    assert!(full.len() == cancelled.len(), "cancel changes the pages");
    for (a, m) in full.iter().zip(&cancelled) {
        assert!(m.is_prefix_of(a), "cancelled decode is not a prefix");
        assert!(
            m.err == Some(Error::Cancelled) || m == a,
            "cancel from the callback"
        );
    }
    let pooled = budget(|| outs(data, 4, usize::MAX, usize::MAX, Some(&POOL)));
    assert!(pooled == full, "a caller's pool changes the result");
});
