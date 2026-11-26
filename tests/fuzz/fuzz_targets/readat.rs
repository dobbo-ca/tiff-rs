#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use tiff::{Error, ReadAt};

#[path = "../common.rs"]
mod common;
use common::{Out, PageRes, Res, TIGHT, run_file};

/// Returns at most `k` bytes per read.
struct Short<'a>(&'a [u8], usize);

impl ReadAt for Short<'_> {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        let n = buf.len().min(self.1);
        self.0.read_at(&mut buf[..n], offset)
    }
}

/// Fails every read from the `n`th on.
struct Failing<'a>(&'a [u8], AtomicUsize);

impl ReadAt for Failing<'_> {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        // ReadAt is Sync: take one read off the count atomically; at 0 the read fails.
        if self
            .1
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
            .is_err()
        {
            return Err(io::Error::other("injected"));
        }
        self.0.read_at(buf, offset)
    }
}

/// What a decode that failed on a read may deliver: the same pixels, or a prefix of them with an
/// `Io` error.
fn prefix_of(got: &Out, want: &Out) -> bool {
    got.is_prefix_of(want) && (got == want || matches!(got.err, Some(Error::Io(_))))
}

// data = [file][read size][fail after]; the file is all of `data`, so a plain TIFF is a seed. A source that returns short reads gives the result of the
// slice; one that fails after n reads gives the same result, or an Open error or a page or band
// error of kind Io with the pixels delivered so far a prefix of the slice's, never a panic.
fuzz_target!(|data: &[u8]| {
    let Some(&[k, fail]) = data.last_chunk::<2>() else {
        return;
    };
    let file = data;
    let size = file.len() as u64;
    let want = run_file(file, size, &TIGHT);
    let got = run_file(Short(file, usize::from(k % 16) + 1), size, &TIGHT);
    assert!(want == got, "short reads change the result");
    let failed = run_file(
        Failing(file, AtomicUsize::new(usize::from(fail))),
        size,
        &TIGHT,
    );
    if failed == want {
        return;
    }
    match (&failed, &want) {
        (Res::Open(Error::Io(_)), _) => {}
        (Res::Pages(_, g), Res::Pages(_, w)) => {
            // Once a read fails, every later one does, so a page is whole, a prefix that ends in
            // an Io error, or not parsed for an Io error.
            for (g, w) in g.iter().zip(w) {
                match (g, w) {
                    (PageRes::Err(Error::Io(_)), _) => {}
                    (PageRes::Ok(_, g), PageRes::Ok(_, w)) => {
                        assert!(prefix_of(g, w), "failing reads change a page");
                    }
                    _ => assert!(g == w, "failing reads change a page"),
                }
            }
        }
        _ => panic!("failing reads change the result"),
    }
});
