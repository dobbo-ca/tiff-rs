#![no_main]
use libfuzzer_sys::fuzz_target;
use std::sync::atomic::AtomicBool;
use tiff::Error;

#[path = "../common.rs"]
mod common;
use common::{MAX_PAGES, PageRes, TIGHT, rows_of, rows_with};

// A File and its Pages are reused: every page is decoded twice, then after a decode that the
// callback stopped and one that was cancelled, then the pages again in reverse order. A decode
// has no state that outlives it: all give the first result, the stopped one is a prefix of it,
// the cancelled one is a prefix or Cancelled.
fuzz_target!(|data: &[u8]| {
    let Ok(f) = tiff::File::open(data, data.len() as u64, &TIGHT) else {
        return;
    };
    let cancel = AtomicBool::new(false);
    let mut first = Vec::new();
    for i in 0..f.num_pages().min(MAX_PAGES) {
        let Ok(p) = f.page(i) else {
            first.push(None);
            continue;
        };
        let a = rows_of(&p, &TIGHT, &cancel, usize::MAX);
        assert!(
            a == rows_of(&p, &TIGHT, &cancel, usize::MAX),
            "second decode differs"
        );
        let s = rows_of(&p, &TIGHT, &cancel, 1);
        assert!(s.is_prefix_of(&a), "stopped decode is not a prefix");
        if a.err.is_none() {
            // a Break after the last band is still a stop
            assert!(s.err == Some(Error::Stopped), "stop ignored");
        }
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        let c = rows_of(&p, &TIGHT, &cancel, usize::MAX);
        cancel.store(false, std::sync::atomic::Ordering::Relaxed);
        assert!(c.is_prefix_of(&a));
        if a.err.is_none() {
            assert!(c.err == Some(Error::Cancelled), "cancel flag ignored");
        }
        assert!(
            a == rows_of(&p, &TIGHT, &cancel, usize::MAX),
            "decode after a cancel differs"
        );
        // A cancel that arrives from the callback, inside the decode: a prefix, then Cancelled
        // (or the whole result if the flag came after the last check).
        let m = rows_with(&p, &TIGHT, &cancel, usize::MAX, 2);
        cancel.store(false, std::sync::atomic::Ordering::Relaxed);
        assert!(m.is_prefix_of(&a));
        assert!(
            m.err == Some(Error::Cancelled) || m == a,
            "mid-decode cancel"
        );
        assert!(
            a == rows_of(&p, &TIGHT, &cancel, usize::MAX),
            "decode after a mid-decode cancel differs"
        );
        first.push(Some((p.info(), a)));
    }
    for i in (0..f.num_pages().min(MAX_PAGES)).rev() {
        let Ok(p) = f.page(i) else { continue };
        let again = PageRes::Ok(p.info(), rows_of(&p, &TIGHT, &cancel, usize::MAX));
        let Some((n, o)) = first[i].take() else {
            panic!("page {i} parsed the second time only");
        };
        assert!(again == PageRes::Ok(n, o), "page {i} differs on reuse");
    }
});
