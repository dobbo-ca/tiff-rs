//! A `rows` call made from inside the callback of another, on the same pool, must finish: a pool
//! worker never waits for the caller. With a pool of one thread and `max_threads` 2, the outer
//! worker would otherwise sit in the full window while the inner call waits for that thread. The
//! shared pool is process state, so the steps of this binary run in one test.

mod common;

use common::{File, Page, collect, pattern};
use std::ops::ControlFlow;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::time::Duration;
use tiff::{Limits, ThreadPool};

/// 256 strips of 4 rows, LZW, so a call has many chunks to hand out.
fn strips() -> Vec<u8> {
    let (w, h) = (64, 1024);
    File {
        pages: vec![Page {
            w,
            h,
            spp: 3,
            bps: 8,
            photometric: 2,
            compression: 5,
            rows_per_strip: 4,
            pix: Some(pattern(w, h, 3, 8)),
            ..Default::default()
        }],
        ..Default::default()
    }
    .bytes()
}

fn open<'a>(b: &'a [u8], lim: &Limits, pool: Option<&ThreadPool>) -> tiff::File<&'a [u8]> {
    let f = tiff::File::open(b, b.len() as u64, lim).unwrap();
    match pool {
        Some(p) => f.with_pool(p.clone()),
        None => f,
    }
}

/// Runs the outer call, whose callback runs the inner call on every band, on a thread of its own
/// with 10 s to finish; both must give the bands of a serial call.
fn nested(max_threads: usize, pool: Option<ThreadPool>) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let (outer, inner) = (strips(), strips());
        let serial = Limits {
            max_threads: 1,
            ..Default::default()
        };
        let want = collect(outer.as_slice(), outer.len() as u64, &serial, None).0;
        let lim = Limits {
            max_threads,
            ..Default::default()
        };
        let fo = open(&outer, &lim, pool.as_ref());
        let fi = open(&inner, &lim, pool.as_ref());
        let (po, pi) = (fo.page(0).unwrap(), fi.page(0).unwrap());
        let (mut got, mut calls) = (Vec::new(), 0);
        po.rows(&AtomicBool::new(false), |y, band| {
            got.push((y, band.to_vec()));
            if calls < 3 {
                // Let the workers fill the window while this thread is in the callback.
                std::thread::sleep(Duration::from_millis(20));
                let mut again = Vec::new();
                pi.rows(&AtomicBool::new(false), |y, b| {
                    again.push((y, b.to_vec()));
                    ControlFlow::Continue(())
                })
                .unwrap();
                assert!(again == want, "the nested call's bands differ");
            }
            calls += 1;
            ControlFlow::Continue(())
        })
        .unwrap();
        assert!(got == want, "the outer call's bands differ");
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("a rows call from inside the callback did not finish within 10 s");
}

#[test]
fn rows_from_the_callback_finishes() {
    // A pool of one thread and two threads for the call.
    nested(2, Some(ThreadPool::new(1).unwrap()));
    // The shared pool, with a call that wants a thread more than it has.
    let n = std::thread::available_parallelism().map_or(1, |n| n.get());
    nested(n + 1, None);
}
