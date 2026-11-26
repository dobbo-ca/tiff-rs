//! Ports of tiff-go's jpeg4_test.go, jpeg_progressive_test.go and the JPEG parts of
//! libtiff_test.go and internal/codec/jpeg_test.go.

mod common;

use common::{File, Page, decode, jpeg_rgb, marked, pattern, ycc_jpeg};
use std::fs;
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use tiff::{Error, Limits};

fn synthetic(name: &str) -> Vec<u8> {
    fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/synthetic")
            .join(name),
    )
    .unwrap()
}

// tests/data/synthetic/cmyk_jpeg_*.tif are 4-component JPEG pages written by libtiff (Pillow,
// tiffcp); the .raw next to each is what Pillow (libtiff) reads from it. libtiff does no colour
// conversion for them, so the samples must be equal.
#[test]
fn jpeg_cmyk_matches_libtiff() {
    for name in ["strip1", "bs", "bt"] {
        let b = synthetic(&format!("cmyk_jpeg_{name}.tif"));
        let want = synthetic(&format!("cmyk_jpeg_{name}.raw"));
        for lim in [
            Limits::default(),
            Limits {
                max_band_bytes: 9000,
                ..Default::default()
            },
        ] {
            let (got, _) = decode(&b, &lim, 0).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(
                got == want,
                "{name} band={}: differs from libtiff",
                lim.max_band_bytes
            );
        }
    }
}

fn cmyk_page(chunk: Vec<u8>) -> Vec<u8> {
    let p = Page {
        w: 70,
        h: 16,
        spp: 4,
        bps: 8,
        photometric: 5,
        compression: 7,
        rows_per_strip: 16,
        chunks: Some(vec![chunk]),
        ..Default::default()
    };
    File {
        pages: vec![p],
        ..Default::default()
    }
    .bytes()
}

// A restart interval changes nothing: jpegtran -restart of the plain stream is the same samples.
#[test]
fn jpeg_cmyk_restart() {
    let got: Vec<Vec<u8>> = ["plain", "restart"]
        .iter()
        .map(|n| {
            let b = cmyk_page(synthetic(&format!("cmyk_jpeg_{n}.jpg")));
            decode(&b, &Limits::default(), 0)
                .unwrap_or_else(|e| panic!("{n}: {e}"))
                .0
        })
        .collect();
    assert!(
        got[0].len() == 70 * 16 * 4 && got[0] == got[1],
        "restart stream decodes differently"
    );
}

// Truncated entropy data and a marker where data should be are corrupt, not a panic.
#[test]
fn jpeg_cmyk_corrupt() {
    let j = synthetic("cmyk_jpeg_plain.jpg");
    let mut marker = j[..j.len() / 2].to_vec();
    marker.extend([0xff, 0xd9]);
    for c in [
        j[..j.len() / 2].to_vec(),
        j[..j.len() - 200].to_vec(),
        marker,
    ] {
        assert!(
            decode(&cmyk_page(c), &Limits::default(), 0).is_err(),
            "accepted"
        );
    }
}

/// A TIFF of one JPEG strip with the given photometric (6: the stream's markers decide; 2: RGB
/// as stored), and the RGB the jpeg decoder gives for it.
fn progressive_tiff(name: &str, photometric: usize) -> (Vec<u8>, Vec<u8>) {
    let j = synthetic(&format!("{name}.jpg"));
    let model = if photometric == 2 {
        jpeg::ColorModel::Rgb
    } else {
        jpeg::ColorModel::Auto
    };
    let want = jpeg_rgb(&j, model);
    let cfg = jpeg::decode_config(&j, None, &jpeg::Limits::default()).unwrap();
    let p = Page {
        w: cfg.width,
        h: cfg.height,
        spp: 3,
        bps: 8,
        photometric,
        compression: 7,
        rows_per_strip: cfg.height,
        chunks: Some(vec![j]),
        ..Default::default()
    };
    (
        File {
            pages: vec![p],
            ..Default::default()
        }
        .bytes(),
        want,
    )
}

// A progressive strip over a band decodes by rows from one reader and equals the whole-frame
// decode.
#[test]
fn jpeg_progressive_strip_bands() {
    for name in ["jpeg_progressive_444", "jpeg_progressive_420"] {
        let (b, want) = progressive_tiff(name, 6);
        let lim = Limits {
            max_band_bytes: 3 * 40 * 8,
            ..Default::default()
        };
        let (got, max_band) = decode(&b, &lim, 0).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(
            got == want && max_band <= 3 * 40 * 8,
            "{name}: equal {}, band {max_band}",
            got == want
        );
    }
}

// Rows gives the row reader's budget back on every exit, so the next decode fits the same
// limits (Go: a process-wide jpeg budget; here a `max_alloc` that fits one frame).
#[test]
fn rows_release_on_early_exit() {
    let (b, _) = progressive_tiff("jpeg_progressive_420", 6);
    let j = synthetic("jpeg_progressive_420.jpg");
    let need = jpeg::row_reader_need(&j, None, &jpeg::Limits::default()).unwrap();
    let lim = Limits {
        max_band_bytes: 3 * 40 * 8,
        max_alloc: need + (1 << 18),
        ..Default::default()
    };
    let f = tiff::File::open(b.as_slice(), b.len() as u64, &lim).unwrap();
    let p = f.page(0).unwrap();
    let cancel = AtomicBool::new(false);
    assert_eq!(
        p.rows(&cancel, |_, _| ControlFlow::Break(())),
        Err(Error::Stopped)
    );
    let flag = AtomicBool::new(false);
    assert_eq!(
        p.rows(&flag, |_, _| {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
            ControlFlow::Continue(())
        }),
        Err(Error::Cancelled)
    );
    p.rows(&cancel, |_, _| ControlFlow::Continue(())).unwrap();
}

// A refused budget is a limit, not corruption (a caller handles them apart).
#[test]
fn jpeg_limit_is_not_corrupt() {
    // SOI, SOF2, SOS: a progressive frame with no entropy data. The row reader reserves its
    // coefficients at SOF, before any data, and the limit refuses them.
    for (w, h, max_alloc) in [
        (2048usize, 2048usize, 20u64 << 20),
        (30000, 30000, Limits::default().max_alloc),
    ] {
        let mut j = vec![
            0xff,
            0xd8,
            0xff,
            0xc2,
            0,
            17,
            8,
            (h >> 8) as u8,
            h as u8,
            (w >> 8) as u8,
            w as u8,
        ];
        j.extend([
            3, 1, 0x11, 0, 2, 0x11, 0, 3, 0x11, 0, 0xff, 0xda, 0, 12, 3, 1, 0, 2, 0, 3, 0, 0, 0, 0,
        ]);
        let p = Page {
            w,
            h,
            spp: 3,
            bps: 8,
            photometric: 2,
            compression: 7,
            rows_per_strip: h,
            chunks: Some(vec![j]),
            pix: Some(vec![0]),
            ..Default::default()
        };
        let b = File {
            pages: vec![p],
            ..Default::default()
        }
        .bytes();
        let lim = Limits {
            max_pixels: 1 << 40,
            max_alloc,
            ..Default::default()
        };
        let r = decode(&b, &lim, 0);
        assert!(
            matches!(r, Err(Error::Limit { .. })),
            "{w}x{h}: {:?}",
            r.err()
        );
    }
}

// jpeg-rs decodes only baseline interleaved (or single-component) frames a band at a time, so
// any other frame is decoded whole per band. It must still decode, and to what one unbanded
// decode gives.
#[test]
fn jpeg_any_frame_bands() {
    for name in [
        "jpeg_baseline",
        "jpeg_noninterleaved",
        "jpeg_progressive_fill",
    ] {
        let j = synthetic(&format!("{name}.jpg"));
        let p = Page {
            w: 16,
            h: 64,
            spp: 3,
            bps: 8,
            photometric: 6,
            compression: 7,
            rows_per_strip: 64,
            chunks: Some(vec![j]),
            ..Default::default()
        };
        let b = File {
            pages: vec![p],
            ..Default::default()
        }
        .bytes();
        let (want, _) = decode(&b, &Limits::default(), 0).unwrap_or_else(|e| panic!("{name}: {e}"));
        let lim = Limits {
            max_band_bytes: 4 * 16 * 8,
            ..Default::default()
        };
        let (got, _) = decode(&b, &lim, 0).unwrap_or_else(|e| panic!("{name}: banded {e}"));
        assert!(got == want, "{name}: banded differs");
    }
}

const RW: usize = 64;
const RH: usize = 48;

/// A `RW` by `RH` page with the given photometric whose chunks (strips of 16 rows, or tiles of
/// 32 by 16) are [`ycc_jpeg`] streams of `pattern`, variant `mark` of [`marked`]; and the chunks.
fn ycc_page(
    photometric: usize,
    tiles: bool,
    chroma444: bool,
    mark: usize,
) -> (Vec<u8>, Vec<Vec<u8>>) {
    let pix = pattern(RW, RH, 3, 8);
    let cw = if tiles { 32 } else { RW };
    let mut chunks = Vec::new();
    for y in (0..RH).step_by(16) {
        for x in (0..RW).step_by(cw) {
            let mut rgb = Vec::new();
            for r in y..y + 16 {
                rgb.extend_from_slice(&pix[3 * (r * RW + x)..3 * (r * RW + x + cw)]);
            }
            chunks.push(
                marked(&ycc_jpeg(&rgb, cw, 16, chroma444))
                    .swap_remove(mark)
                    .1,
            );
        }
    }
    let mut p = Page {
        w: RW,
        h: RH,
        spp: 3,
        bps: 8,
        photometric,
        compression: 7,
        rows_per_strip: 16,
        chunks: Some(chunks.clone()),
        ..Default::default()
    };
    if tiles {
        (p.rows_per_strip, p.tile_w, p.tile_l) = (0, 32, 16);
    }
    let b = File {
        pages: vec![p],
        ..Default::default()
    }
    .bytes();
    (b, chunks)
}

/// The page of [`ycc_page`] as jpeg-rs reads its chunks with `model`.
fn page_rgb(chunks: &[Vec<u8>], tiles: bool, model: jpeg::ColorModel) -> Vec<u8> {
    let cw = if tiles { 32 } else { RW };
    let mut v = vec![0u8; 3 * RW * RH];
    for (i, c) in chunks.iter().enumerate() {
        let px = jpeg_rgb(c, model);
        let (x0, y0) = (i % (RW / cw) * cw, i / (RW / cw) * 16);
        for r in 0..16 {
            v[3 * ((y0 + r) * RW + x0)..][..3 * cw].copy_from_slice(&px[3 * cw * r..][..3 * cw]);
        }
    }
    v
}

// Photometric RGB: the samples of a 4:4:4 stream are RGB as stored, whatever its JFIF or Adobe
// markers say (libtiff, OpenSlide; Aperio SVS). Photometric YCbCr: the markers decide.
#[test]
fn jpeg_photometric_rgb_is_as_stored() {
    let cancel = AtomicBool::new(false);
    for tiles in [false, true] {
        for mark in 0..3 {
            let at = format!("tiles {tiles} mark {mark}");
            let (rgb, chunks) = ycc_page(2, tiles, true, mark);
            let (ycc, _) = ycc_page(6, tiles, true, mark);
            assert!(
                page_rgb(&chunks, tiles, jpeg::ColorModel::Rgb)
                    != page_rgb(&chunks, tiles, jpeg::ColorModel::Auto),
                "{at}: the models agree"
            );
            for (b, model) in [
                (&rgb, jpeg::ColorModel::Rgb),
                (&ycc, jpeg::ColorModel::Auto),
            ] {
                let want = page_rgb(&chunks, tiles, model);
                for max_threads in [1, 4] {
                    let at = format!("{at} {model:?} threads {max_threads}");
                    let lim = Limits {
                        max_threads,
                        ..Default::default()
                    };
                    let (got, _) = decode(b, &lim, 0).unwrap_or_else(|e| panic!("{at}: {e}"));
                    assert!(got == want, "{at}: rows differ");
                    let f = tiff::File::open(b.as_slice(), b.len() as u64, &lim).unwrap();
                    let page = f.page(0).unwrap();
                    let (mut dec, mut out) = (tiff::Decoder::new(), Vec::new());
                    for (i, c) in chunks.iter().enumerate() {
                        page.decode_chunk_into(i, &mut dec, &mut out, &cancel)
                            .unwrap_or_else(|e| panic!("{at} chunk {i}: {e}"));
                        assert!(out == jpeg_rgb(c, model), "{at} chunk {i}: differs");
                    }
                }
            }
        }
    }
}

// libtiff refuses an RGB JPEG whose components are subsampled ("Improper JPEG sampling
// factors"): unsupported from rows and from a chunk. The same stream under YCbCr decodes.
#[test]
fn jpeg_photometric_rgb_subsampled_is_unsupported() {
    let cancel = AtomicBool::new(false);
    for tiles in [false, true] {
        let (rgb, chunks) = ycc_page(2, tiles, false, 0);
        let r = decode(&rgb, &Limits::default(), 0);
        assert!(
            matches!(r, Err(Error::Unsupported(_))),
            "tiles {tiles}: {:?}",
            r.err()
        );
        let f = tiff::File::open(rgb.as_slice(), rgb.len() as u64, &Limits::default()).unwrap();
        let r = f.page(0).unwrap().decode_chunk_into(
            0,
            &mut tiff::Decoder::new(),
            &mut Vec::new(),
            &cancel,
        );
        assert!(
            matches!(r, Err(Error::Unsupported(_))),
            "tiles {tiles}: {r:?}"
        );
        let (ycc, _) = ycc_page(6, tiles, false, 0);
        let (got, _) = decode(&ycc, &Limits::default(), 0).unwrap();
        assert!(
            got == page_rgb(&chunks, tiles, jpeg::ColorModel::Auto),
            "tiles {tiles}: ycbcr differs"
        );
    }
}

// A progressive 4:4:4 strip (jpeg-rs's row reader, whole and over bands) under Photometric RGB is
// RGB as stored too.
#[test]
fn jpeg_photometric_rgb_progressive() {
    let (b, want) = progressive_tiff("jpeg_progressive_444", 2);
    for lim in [
        Limits::default(),
        Limits {
            max_band_bytes: 3 * 40 * 8,
            ..Default::default()
        },
    ] {
        let (got, _) = decode(&b, &lim, 0).unwrap_or_else(|e| panic!("{e}"));
        assert!(got == want, "band {}: differs", lim.max_band_bytes);
    }
}
