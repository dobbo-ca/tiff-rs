//! `Page::decode_chunk_into`: for every fixture file, page and chunk, the chunks cropped to the
//! image and put together are the bytes `Page::rows` delivers, with one `Decoder` for all of
//! them; and the errors and limits of a single chunk, which on a mutated one-chunk page are those
//! of `rows`.

mod common;

use common::{Page, pattern};
use std::fs;
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use tiff::{Decoder, Error, File, Limits};

fn is_send<T: Send>() {}

/// The `.tif` files of `tests/data/golden` and `tests/data/synthetic`, in name order.
fn fixture_files() -> Vec<(String, Vec<u8>)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let mut v = Vec::new();
    for dir in ["golden", "synthetic"] {
        for e in fs::read_dir(root.join(dir)).unwrap() {
            let path = e.unwrap().path();
            if path.extension().is_some_and(|x| x == "tif") {
                let name = format!("{dir}/{}", path.file_name().unwrap().to_string_lossy());
                v.push((name, fs::read(&path).unwrap()));
            }
        }
    }
    v.sort();
    v
}

#[test]
fn chunks_assemble_to_rows() {
    is_send::<Decoder>();
    let cancel = AtomicBool::new(false);
    // One decoder for every file, page and codec.
    let mut dec = Decoder::new();
    let mut chunk = Vec::new();
    let (mut pages, mut chunks) = (0, 0);
    for (name, b) in fixture_files() {
        let f = File::open(b.as_slice(), b.len() as u64, &Limits::default()).unwrap();
        for pi in 0..f.num_pages() {
            let Ok(page) = f.page(pi) else { continue };
            let info = page.info();
            let rb = info.row_bytes as usize;
            let mut want = Vec::new();
            let r = page.rows(&cancel, |y, band| {
                assert_eq!(y * rb, want.len(), "{name}: bands are in order");
                want.extend_from_slice(band);
                ControlFlow::Continue(())
            });
            // A page that `rows` refuses (the CCITT files with 2-D coding or fill bits) has no
            // image to compare with.
            if r.is_err() {
                continue;
            }
            assert_eq!(want.len(), rb * info.height, "{name}");

            let lay = page.layout();
            assert_eq!(
                lay.chunks,
                lay.across as usize * lay.down as usize,
                "{name}"
            );
            if !lay.tiled {
                assert_eq!((lay.across, lay.chunk_width as usize), (1, info.width));
                assert!(lay.chunk_height as usize <= info.height, "{name}");
            }
            // Bytes of one chunk row.
            let crb = (lay.chunk_width as usize * info.samples_per_pixel * info.bits_per_sample)
                .div_ceil(8);
            let mut got = vec![0u8; want.len()];
            for i in 0..lay.chunks {
                page.decode_chunk_into(i, &mut dec, &mut chunk, &cancel)
                    .unwrap_or_else(|e| panic!("{name} page {pi} chunk {i}: {e}"));
                let (ty, tx) = (i / lay.across as usize, i % lay.across as usize);
                let y0 = ty * lay.chunk_height as usize;
                let seen = (info.height - y0).min(lay.chunk_height as usize);
                // Every row of a chunk as stored: a tile is whole, a strip has what remains.
                let stored = if lay.tiled {
                    lay.chunk_height as usize
                } else {
                    seen
                };
                assert_eq!(
                    chunk.len(),
                    stored * crb,
                    "{name} page {pi} chunk {i}: size"
                );
                for r in 0..seen {
                    let x0 = tx * crb;
                    let n = crb.min(rb - x0);
                    got[(y0 + r) * rb + x0..][..n].copy_from_slice(&chunk[r * crb..][..n]);
                }
                chunks += 1;
            }
            assert!(got == want, "{name} page {pi}: chunks differ from rows");
            pages += 1;
        }
    }
    // The fixtures are all read: tiles and strips of every codec, and CMYK JPEG.
    assert!(
        pages >= 30 && chunks >= pages,
        "{pages} pages, {chunks} chunks"
    );
}

fn grey(path: &str) -> Vec<u8> {
    fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/golden")
            .join(path),
    )
    .unwrap()
}

#[test]
fn errors_and_limits_of_one_chunk() {
    let b = grey("grey_zip_strip.tif");
    let open = |lim: &Limits| File::open(b.as_slice(), b.len() as u64, lim).unwrap();
    let cancel = AtomicBool::new(false);
    let mut dec = Decoder::default();
    let mut out = vec![1, 2, 3];

    let f = open(&Limits::default());
    let page = f.page(0).unwrap();
    let n = page.layout().chunks;
    assert!(n > 1);
    assert!(matches!(
        page.decode_chunk_into(n, &mut dec, &mut out, &cancel),
        Err(Error::Unsupported(_))
    ));
    assert!(out.is_empty(), "an error leaves out empty");

    // The last strip has only the rows that remain.
    page.decode_chunk_into(n - 1, &mut dec, &mut out, &cancel)
        .unwrap();
    let lay = page.layout();
    let rows = page.info().height - (n - 1) * lay.chunk_height as usize;
    assert_eq!(out.len(), rows * page.info().row_bytes as usize);

    // A set cancel flag is checked before the chunk.
    let stop = AtomicBool::new(true);
    assert_eq!(
        page.decode_chunk_into(0, &mut dec, &mut out, &stop),
        Err(Error::Cancelled)
    );

    // A chunk over max_band_bytes, or with no room in max_alloc.
    for (lim, name) in [
        (
            Limits {
                max_band_bytes: 8,
                ..Limits::default()
            },
            "band bytes",
        ),
        (
            Limits {
                max_alloc: 64,
                ..Limits::default()
            },
            "alloc",
        ),
    ] {
        let f = open(&lim);
        let page = f.page(0).unwrap();
        assert!(
            matches!(
                page.decode_chunk_into(0, &mut dec, &mut out, &cancel),
                Err(Error::Limit { name: n, .. }) if n == name
            ),
            "{name}"
        );
    }
}

/// A one-strip page of the given codec: grey or RGB, 32 by 16.
fn one_strip(compression: usize, spp: usize) -> Vec<u8> {
    common::File {
        pages: vec![Page {
            w: 32,
            h: 16,
            spp,
            bps: 8,
            // jpeg-rs writes YCbCr.
            photometric: match (spp, compression) {
                (1, _) => 1,
                (_, 7) => 6,
                _ => 2,
            },
            compression,
            pix: Some(pattern(32, 16, spp, 8)),
            ..Default::default()
        }],
        ..Default::default()
    }
    .bytes()
}

/// A result without what `rows` and a chunk call may word differently: the numbers of a limit.
fn kind(r: &Result<(), Error>) -> String {
    match r {
        Err(Error::Limit { name, .. }) => format!("limit {name}"),
        r => format!("{r:?}"),
    }
}

// The data and reader errors of a chunk are those of `rows` for the same bytes: on one-strip pages
// with a byte changed anywhere in the file, `rows` and `decode_chunk_into` end the same way.
#[test]
fn chunk_errors_are_those_of_rows() {
    let lim = Limits {
        max_pixels: 1 << 16,
        ..Limits::default()
    };
    let cancel = AtomicBool::new(false);
    let mut dec = Decoder::new();
    let mut out = Vec::new();
    let (mut compared, mut failed) = (0, 0);
    for compression in [1, 5, 8, 32773, 7] {
        for spp in [1, 3] {
            let b = one_strip(compression, spp);
            let step = b.len() / 600 + 1;
            for pos in (0..b.len()).step_by(step) {
                for xor in [0xffu8, 0x55] {
                    let mut m = b.clone();
                    m[pos] ^= xor;
                    let Ok(f) = File::open(m.as_slice(), m.len() as u64, &lim) else {
                        continue;
                    };
                    let Ok(page) = f.page(0) else { continue };
                    if page.layout().chunks != 1 {
                        continue;
                    }
                    let r = page.rows(&cancel, |_, _| ControlFlow::Continue(()));
                    let c = page.decode_chunk_into(0, &mut dec, &mut out, &cancel);
                    assert_eq!(
                        kind(&r),
                        kind(&c),
                        "compression {compression} spp {spp}: byte {pos} xor {xor}"
                    );
                    compared += 1;
                    failed += usize::from(r.is_err());
                }
            }
        }
    }
    assert!(
        compared > 500 && failed > 5,
        "{compared} compared, {failed} failed"
    );
}

// A tile past the image edge whose stream codes only the rows inside the image: `rows` reads those
// and is Ok; the chunk call is Ok too, with the padding rows zero.
#[test]
fn bottom_tile_with_visible_rows_only() {
    let visible = pattern(16, 8, 1, 8);
    let b = common::File {
        pages: vec![Page {
            w: 16,
            h: 8,
            spp: 1,
            bps: 8,
            photometric: 1,
            compression: 1,
            tile_w: 16,
            tile_l: 16,
            chunks: Some(vec![visible.clone()]),
            ..Default::default()
        }],
        ..Default::default()
    }
    .bytes();
    let f = File::open(b.as_slice(), b.len() as u64, &Limits::default()).unwrap();
    let page = f.page(0).unwrap();
    let cancel = AtomicBool::new(false);
    let mut got = Vec::new();
    page.rows(&cancel, |_, band| {
        got.extend_from_slice(band);
        ControlFlow::Continue(())
    })
    .unwrap();
    assert!(got == visible);
    let mut out = vec![7u8; 3];
    page.decode_chunk_into(0, &mut Decoder::new(), &mut out, &cancel)
        .unwrap();
    assert_eq!(out.len(), 16 * 16);
    assert!(out[..visible.len()] == visible[..]);
    assert!(out[visible.len()..].iter().all(|&v| v == 0));
}

// A chunk of more pixels than `max_pixels`, though the page passes.
#[test]
fn chunk_over_max_pixels() {
    let b = common::File {
        pages: vec![Page {
            w: 16,
            h: 16,
            spp: 1,
            bps: 8,
            photometric: 1,
            compression: 1,
            tile_w: 256,
            tile_l: 256,
            pix: Some(pattern(16, 16, 1, 8)),
            ..Default::default()
        }],
        ..Default::default()
    }
    .bytes();
    let lim = Limits {
        max_pixels: 1000,
        ..Limits::default()
    };
    let f = File::open(b.as_slice(), b.len() as u64, &lim).unwrap();
    let page = f.page(0).unwrap();
    let mut out = Vec::new();
    let r = page.decode_chunk_into(0, &mut Decoder::new(), &mut out, &AtomicBool::new(false));
    assert!(
        matches!(r, Err(Error::Limit { name: "pixels", .. })),
        "{r:?}"
    );
    assert!(out.is_empty());
}
