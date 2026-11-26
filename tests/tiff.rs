//! Ports of tiff-go's tiff_test.go, hostile_test.go, libtiff_test.go and example_test.go.

mod common;

use common::{File, Page, decode, pattern};
use std::fs;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tiff::{Error, Limits};

fn photometric(spp: usize) -> usize {
    match spp {
        1 => 1,
        3 => 2,
        _ => 5,
    }
}

fn open<'a>(b: &'a [u8], lim: &Limits) -> Result<tiff::File<&'a [u8]>, Error> {
    tiff::File::open(b, b.len() as u64, lim)
}

fn is_limit<T>(e: &Result<T, Error>) -> bool {
    matches!(e, Err(Error::Limit { .. }))
}

fn is_corrupt<T>(e: &Result<T, Error>) -> bool {
    matches!(e, Err(Error::Corrupt(_)))
}

fn is_unsupported<T>(e: &Result<T, Error>) -> bool {
    matches!(e, Err(Error::Unsupported(_)))
}

#[test]
fn round_trip() {
    let formats = [(1, 1), (1, 4), (1, 8), (3, 8), (4, 8), (1, 16), (3, 16)];
    let mut n = 0;
    for comp in [1, 5, 8, 32946, 32773] {
        for (spp, bps) in formats {
            for pred in [0, 2] {
                if pred == 2 && bps < 8 {
                    continue;
                }
                for lay in ["strip1", "strip7", "whole", "tile"] {
                    for le in [false, true] {
                        for big in [false, true] {
                            // BigTIFF: a few shapes are enough
                            if big
                                && (comp != 1 || lay != "whole" || spp != 3)
                                && (!le || bps != 16)
                            {
                                continue;
                            }
                            let name =
                                format!("c{comp}/{spp}x{bps}/p{pred}/{lay}/le={le}/big={big}");
                            let mut p = Page {
                                w: 37,
                                h: 50,
                                spp,
                                bps,
                                photometric: photometric(spp),
                                compression: comp,
                                predictor: pred,
                                ..Default::default()
                            };
                            match lay {
                                "strip1" => p.rows_per_strip = 1,
                                "strip7" => p.rows_per_strip = 7,
                                "tile" => (p.tile_w, p.tile_l) = (16, 16),
                                _ => {}
                            }
                            let pix = pattern(p.w, p.h, spp, bps);
                            p.pix = Some(pix.clone());
                            let b = File {
                                little_endian: le,
                                big,
                                pages: vec![p],
                            }
                            .bytes();
                            let (got, _) = decode(&b, &Limits::default(), 0)
                                .unwrap_or_else(|e| panic!("{name}: {e}"));
                            assert!(got == pix, "{name}: pixels differ");
                            n += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(n > 500, "{n} cases");
}

#[test]
fn multi_page() {
    let mut pages = Vec::new();
    for i in 0..3 {
        let mut p = Page {
            w: 10 + i,
            h: 5 + 2 * i,
            spp: 1,
            bps: 8,
            photometric: 1,
            compression: 5 * (i % 2),
            ..Default::default()
        };
        p.pix = Some(pattern(p.w, p.h, 1, 8));
        pages.push(p);
    }
    let b = File {
        pages: pages.clone(),
        ..Default::default()
    }
    .bytes();
    let f = open(&b, &Limits::default()).unwrap();
    assert_eq!(f.num_pages(), 3);
    for (i, p) in pages.iter().enumerate() {
        let (got, _) = decode(&b, &Limits::default(), i).unwrap();
        assert_eq!(Some(got), p.pix, "page {i}");
    }
    assert!(f.page(3).is_err(), "page 3 should not exist");
}

fn assert_close(got: &[u8], want: &[u8], tol: i32) {
    assert_eq!(got.len(), want.len(), "length");
    for i in 0..got.len() {
        let d = i32::from(got[i]) - i32::from(want[i]);
        assert!(d.abs() <= tol, "byte {i}: {} vs {}", got[i], want[i]);
    }
}

// A single strip far over the band cap decodes in bands, holding a small fraction of its
// decoded size (e2e-tiff-strip-rgb and -jpeg, scaled down). Go measured the allocations; here
// `max_alloc` is set to half the decoded size, so a decoder that held the strip would be refused.
#[test]
fn single_strip_streams() {
    const W: usize = 2000;
    const H: usize = 1000;
    const CAP: u64 = 64 << 10;
    for comp in [1, 5, 8, 32773, 7] {
        let mut p = Page {
            w: W,
            h: H,
            spp: 3,
            bps: 8,
            photometric: if comp == 7 { 6 } else { 2 }, // jpeg-rs writes YCbCr
            compression: comp,
            ..Default::default()
        };
        let pix = pattern(W, H, 3, 8);
        p.pix = Some(pix.clone());
        let b = File {
            pages: vec![p],
            ..Default::default()
        }
        .bytes();
        // JPEG is exempt: it holds its compressed chunk, so the cap there is the default.
        let max_alloc = if comp == 7 {
            Limits::default().max_alloc
        } else {
            3 << 20
        };
        let lim = Limits {
            max_band_bytes: CAP,
            max_alloc,
            ..Default::default()
        };
        let f = open(&b, &lim).unwrap();
        let pg = f.page(0).unwrap();
        let mut max_band = 0;
        pg.rows(&AtomicBool::new(false), |y, rows| {
            max_band = max_band.max(rows.len());
            let want = &pix[y * W * 3..y * W * 3 + rows.len()];
            if comp == 7 {
                assert_close(rows, want, 24);
            } else {
                assert!(rows == want, "comp {comp}: pixels differ at row {y}");
            }
            ControlFlow::Continue(())
        })
        .unwrap_or_else(|e| panic!("comp {comp}: {e}"));
        assert!(max_band as u64 <= CAP, "band {max_band} > cap {CAP}");
    }
}

#[test]
fn jpeg() {
    for lay in ["strips", "tiles"] {
        for spp in [1, 3] {
            let mut p = Page {
                w: 70,
                h: 50,
                spp,
                bps: 8,
                photometric: if spp == 1 { 1 } else { 6 }, // jpeg-rs writes YCbCr
                compression: 7,
                rows_per_strip: 20,
                ..Default::default()
            };
            if lay == "tiles" {
                (p.rows_per_strip, p.tile_w, p.tile_l) = (0, 32, 32);
            }
            // smooth content so lossy error is small
            let pix: Vec<u8> = (0..p.w * spp * p.h)
                .map(|i| (i % (p.w * spp) / spp * 2 + i / (p.w * spp)) as u8)
                .collect();
            p.pix = Some(pix.clone());
            let b = File {
                pages: vec![p.clone()],
                ..Default::default()
            }
            .bytes();
            let (whole, _) = decode(&b, &Limits::default(), 0).unwrap();
            assert_close(&whole, &pix, 12);
            let lim = Limits {
                max_band_bytes: (p.w * spp * 8 * 4) as u64,
                ..Default::default()
            };
            let (banded, _) = decode(&b, &lim, 0).unwrap();
            assert!(
                banded == whole,
                "{lay}{spp}: banded decode differs from whole-chunk decode"
            );
        }
    }
}

#[test]
fn info_palette() {
    let cm: Vec<u16> = (0..48).map(|i| i * 1000).collect();
    let p = Page {
        w: 8,
        h: 8,
        spp: 1,
        bps: 4,
        photometric: 3,
        color_map: Some(cm),
        ..Default::default()
    };
    let b = File {
        little_endian: true,
        pages: vec![p],
        ..Default::default()
    }
    .bytes();
    let f = open(&b, &Limits::default()).unwrap();
    let pg = f.page(0).unwrap();
    let i = pg.info();
    assert!(
        i.width == 8
            && i.height == 8
            && i.bits_per_sample == 4
            && i.photometric == tiff::PALETTE
            && i.color_map.len() == 48
            && i.color_map[47] == 47000,
        "info {i:?}"
    );
}

// Group 4: an all-white row (V0), an all-black row (horizontal mode), the same row again
// (V0 V0), then EOFB.
#[test]
fn ccitt() {
    let mut bits =
        String::from("1") + "001" + "00110101" + "000101" + "11" + "000000000001000000000001";
    while !bits.len().is_multiple_of(8) {
        bits.push('0');
    }
    let data: Vec<u8> = bits
        .as_bytes()
        .chunks(8)
        .map(|c| c.iter().fold(0u8, |v, &b| v << 1 | (b - b'0')))
        .collect();
    // libtiff returns the decoded bits as they are, whatever the photometric.
    for photo in [1, 0] {
        let p = Page {
            w: 8,
            h: 3,
            spp: 1,
            bps: 1,
            photometric: photo,
            compression: 4,
            chunks: Some(vec![data.clone()]),
            ..Default::default()
        };
        let b = File {
            pages: vec![p],
            ..Default::default()
        }
        .bytes();
        let (got, _) = decode(&b, &Limits::default(), 0).unwrap();
        assert_eq!(got, [0x00, 0xff, 0xff], "photometric {photo}");
    }
}

#[test]
fn limits() {
    let page = |w, h| Page {
        w,
        h,
        spp: 1,
        bps: 8,
        photometric: 1,
        ..Default::default()
    };
    let many = |n: usize| {
        File {
            pages: vec![page(1, 1); n],
            ..Default::default()
        }
        .bytes()
    };
    // ifd count
    let b = many(10000);
    open(&b, &Limits::default()).unwrap_or_else(|e| panic!("10000 IFDs: {e}"));
    let b = many(10001);
    match open(&b, &Limits::default()) {
        Err(Error::Limit { name: "ifds", .. }) => {}
        r => panic!("10001 IFDs: {:?}", r.err()),
    }
    // ifd loop
    let mut b = File {
        pages: vec![page(1, 1), page(1, 1)],
        ..Default::default()
    }
    .bytes();
    let n = b.len();
    let first = b[4..8].to_vec();
    b[n - 4..].copy_from_slice(&first); // last IFD's next = first IFD
    assert!(is_corrupt(&open(&b, &Limits::default())), "loop");
    // pixels
    let b = File {
        pages: vec![page(1000, 1000)],
        ..Default::default()
    }
    .bytes();
    let lim = Limits {
        max_pixels: 999_999,
        ..Default::default()
    };
    let f = open(&b, &lim).unwrap();
    assert!(matches!(
        f.page(0),
        Err(Error::Limit { name: "pixels", .. })
    ));
    // defaults refuse a huge header
    let mut p = page(1 << 31, 1 << 31);
    (p.chunks, p.pix) = (Some(vec![vec![0]]), Some(vec![0]));
    let b = File {
        pages: vec![p],
        ..Default::default()
    }
    .bytes();
    let f = open(&b, &Limits::default()).unwrap();
    assert!(is_limit(&f.page(0).map(|_| ())), "huge");
    // row wider than the band
    let b = File {
        pages: vec![page(100, 4)],
        ..Default::default()
    }
    .bytes();
    let lim = Limits {
        max_band_bytes: 99,
        ..Default::default()
    };
    assert!(is_limit(&decode(&b, &lim, 0)), "band");
    // tile row wider than the band
    let mut p = page(64, 64);
    (p.tile_w, p.tile_l) = (16, 16);
    let b = File {
        pages: vec![p],
        ..Default::default()
    }
    .bytes();
    let lim = Limits {
        max_band_bytes: 64 * 16 - 1,
        ..Default::default()
    };
    assert!(is_limit(&decode(&b, &lim, 0)), "tile band");
    // working memory
    let p = Page {
        w: 100,
        h: 100,
        spp: 1,
        bps: 8,
        photometric: 1,
        ..Default::default()
    };
    let b = File {
        pages: vec![p],
        ..Default::default()
    }
    .bytes();
    let lim = Limits {
        max_alloc: 99,
        ..Default::default()
    };
    assert!(
        matches!(decode(&b, &lim, 0), Err(Error::Limit { name: "alloc", .. })),
        "alloc"
    );
}

#[test]
fn refusals() {
    let base = Page {
        w: 8,
        h: 8,
        spp: 1,
        bps: 8,
        photometric: 1,
        ..Default::default()
    };
    type Mutation = fn(&mut Page);
    let muts: [(&str, Mutation); 4] = [
        ("lab", |p| p.photometric = 8),
        ("old jpeg", |p| p.compression = 6),
        ("float pred", |p| p.predictor = 3),
        ("pred at 4bit", |p| (p.predictor, p.bps) = (2, 4)),
    ];
    for (name, m) in muts {
        let mut p = base.clone();
        m(&mut p);
        p.chunks = Some(vec![vec![0; 64]]);
        let b = File {
            pages: vec![p],
            ..Default::default()
        }
        .bytes();
        assert!(is_unsupported(&decode(&b, &Limits::default(), 0)), "{name}");
    }
    for comp in [1, 5, 8, 32773] {
        let mut p = base.clone();
        p.compression = comp;
        p.chunks = Some(vec![vec![1, 2]]);
        let b = File {
            pages: vec![p],
            ..Default::default()
        }
        .bytes();
        let r = decode(&b, &Limits::default(), 0);
        assert!(is_corrupt(&r), "comp {comp}: {:?}", r.err());
    }
    assert!(is_corrupt(&open(b"GIF89a..", &Limits::default())));
}

#[test]
fn cancel() {
    let p = Page {
        w: 100,
        h: 100,
        spp: 1,
        bps: 8,
        photometric: 1,
        rows_per_strip: 10,
        ..Default::default()
    };
    let b = File {
        pages: vec![p],
        ..Default::default()
    }
    .bytes();
    let f = open(&b, &Limits::default()).unwrap();
    let pg = f.page(0).unwrap();
    let cancel = AtomicBool::new(false);
    let mut bands = 0;
    let r = pg.rows(&cancel, |_, _| {
        bands += 1;
        cancel.store(true, Ordering::Relaxed);
        ControlFlow::Continue(())
    });
    assert_eq!((r, bands), (Err(Error::Cancelled), 1));
    // a callback that breaks stops the decode
    let cancel = AtomicBool::new(false);
    assert_eq!(
        pg.rows(&cancel, |_, _| ControlFlow::Break(())),
        Err(Error::Stopped)
    );
}

fn raw(data: &[u8], tags: &[(u16, u16, u32)]) -> Vec<u8> {
    let mut b = b"II*\x00\x00\x00\x00\x00".to_vec();
    b.extend(data);
    let n = b.len() as u32;
    b[4..8].copy_from_slice(&n.to_le_bytes());
    b.extend((tags.len() as u16).to_le_bytes());
    for &(id, typ, v) in tags {
        b.extend(id.to_le_bytes());
        b.extend(typ.to_le_bytes());
        b.extend(1u32.to_le_bytes());
        b.extend(v.to_le_bytes());
    }
    b.extend(0u32.to_le_bytes());
    b
}

#[test]
fn hostile_headers() {
    let lim = Limits {
        max_ifds: 64,
        max_pixels: 1 << 22,
        max_band_bytes: 1 << 20,
        ..Default::default()
    };
    type Tags = Vec<(u16, u16, u32)>;
    let cases: [(&str, Tags, bool); 2] = [
        (
            "rows per strip 0",
            vec![
                (256, 4, 4),
                (257, 4, 4),
                (262, 3, 1),
                (273, 4, 8),
                (278, 4, 0),
                (279, 4, 16),
            ],
            false,
        ),
        (
            "tile wider than a band",
            vec![
                (256, 4, 1),
                (257, 4, 1),
                (262, 3, 2),
                (277, 3, 3),
                (322, 4, 1 << 28),
                (323, 4, 1),
                (324, 4, 8),
                (325, 4, 16),
            ],
            true,
        ),
    ];
    for (name, tags, want_limit) in cases {
        let b = raw(&[0; 16], &tags);
        let r = decode(&b, &lim, 0);
        assert!(
            if want_limit {
                is_limit(&r)
            } else {
                is_corrupt(&r)
            },
            "{name}: {:?}",
            r.err()
        );
    }
}

fn testdata(parts: &[&str]) -> PathBuf {
    let mut p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    p.extend(parts);
    p
}

#[test]
fn oracle_files() {
    let read = |n: &str| fs::read(testdata(&["golden", n])).unwrap();
    let tif = read("bw.tif");
    let truth = &tif[8..8 + 4082]; // tiffcp from this uncompressed file made the others
    for n in ["bw_g4_r16.tif", "bwmw_g4_r16.tif"] {
        let b = read(n);
        let (got, _) = decode(&b, &Limits::default(), 0).unwrap_or_else(|e| panic!("{n}: {e}"));
        for y in 0..157 {
            // ignore the pad bits of each row's last byte
            for x in 0..26 {
                let m = if x == 25 { 0xe0 } else { 0xff };
                let i = y * 26 + x;
                assert!(
                    got[i] & m == truth[i] & m,
                    "{n}: differs from libtiff at row {y} byte {x}"
                );
            }
        }
    }
    for n in ["bw_g3pfill_r16.tif", "bw_g3p2d_r16.tif"] {
        assert!(
            is_unsupported(&decode(&read(n), &Limits::default(), 0)),
            "{n}"
        );
    }
}

#[test]
fn fill_order_2_at_8_bits() {
    let b = raw(
        &[7, 0, 0, 0],
        &[
            (256, 4, 1),
            (257, 4, 1),
            (258, 3, 8),
            (262, 3, 1),
            (266, 3, 2),
            (273, 4, 8),
            (278, 4, 1),
            (279, 4, 1),
        ],
    );
    let (got, _) = decode(&b, &Limits::default(), 0).unwrap();
    assert_eq!(got, [7]);
}

// tests/data/golden/<src>_<codec>_<strip|tile>.tif were written by libtiff's tiffcp from
// <src>.raw's image (37x29); lossless files must decode to <src>.raw exactly, JPEG ones to
// libtiff's own decode (<name>.ref) within IDCT rounding.
#[test]
fn libtiff_oracle() {
    let mut files: Vec<PathBuf> = fs::read_dir(testdata(&["golden"]))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let n = p.file_name().unwrap().to_string_lossy().into_owned();
            n.ends_with(".tif") && n.matches('_').count() >= 2
        })
        .collect();
    files.sort();
    assert!(files.len() >= 30, "only {} oracle files", files.len());
    for path in files {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        if name.starts_with("bw") {
            continue;
        }
        let (want, jpeg) = match fs::read(path.with_extension("ref")) {
            Ok(w) => (w, true),
            Err(_) => {
                let src = name.split('_').next().unwrap();
                let raw = format!("{src}.raw");
                (
                    fs::read(testdata(&["golden", raw.as_str()])).unwrap(),
                    false,
                )
            }
        };
        let b = fs::read(&path).unwrap();
        for lim in [
            Limits::default(),
            Limits {
                max_band_bytes: 2048,
                ..Default::default()
            },
        ] {
            let got = match decode(&b, &lim, 0) {
                Ok((g, _)) => g,
                Err(e) => {
                    panic!("{name} band={}: {e}", lim.max_band_bytes);
                }
            };
            assert_eq!(got.len(), want.len(), "{name}: length");
            let maxd = got
                .iter()
                .zip(&want)
                .map(|(&a, &b)| (i32::from(a) - i32::from(b)).abs())
                .max()
                .unwrap_or(0);
            assert!(
                (jpeg || got == want) && maxd <= 8,
                "{name} band={}: differs from libtiff (max diff {maxd})",
                lim.max_band_bytes
            );
        }
    }
}

#[test]
fn example() {
    let p = Page {
        w: 4,
        h: 2,
        spp: 1,
        bps: 8,
        photometric: tiff::BLACK_IS_ZERO,
        compression: 5,
        ..Default::default()
    };
    let b = File {
        pages: vec![p.clone(), p],
        ..Default::default()
    }
    .bytes();
    let f = tiff::File::open(b.as_slice(), b.len() as u64, &Limits::default()).unwrap();
    let mut out = Vec::new();
    for i in 0..f.num_pages() {
        let pg = f.page(i).unwrap();
        let mut rows = 0;
        let rb = pg.info().row_bytes as usize;
        pg.rows(&AtomicBool::new(false), |_, band| {
            rows += band.len() / rb;
            ControlFlow::Continue(())
        })
        .unwrap();
        out.push(format!("page {i} rows {rows}"));
    }
    assert_eq!(out, ["page 0 rows 2", "page 1 rows 2"]);
}

// Go's FuzzTIFF seeds (the fixture files, and tests/data/fuzz/FuzzTIFF), plus deterministic
// mutations of them, under tight limits: nothing may panic or hold more than the limits allow.
fn unquote_seed(s: &str) -> Vec<u8> {
    let body = s
        .lines()
        .nth(1)
        .unwrap()
        .strip_prefix("[]byte(\"")
        .unwrap()
        .strip_suffix("\")")
        .unwrap();
    let b = body.as_bytes();
    let (mut out, mut i) = (Vec::new(), 0);
    while i < b.len() {
        if b[i] != b'\\' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        i += 1;
        let c = b[i];
        i += 1;
        match c {
            b'x' => {
                out.push(u8::from_str_radix(&body[i..i + 2], 16).unwrap());
                i += 2;
            }
            b'0'..=b'7' => {
                out.push(u8::from_str_radix(&body[i - 1..i + 2], 8).unwrap());
                i += 2;
            }
            b'a' => out.push(7),
            b'b' => out.push(8),
            b'f' => out.push(12),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'v' => out.push(11),
            c => out.push(c),
        }
    }
    out
}

fn run_all(b: &[u8], lim: &Limits) {
    if let Ok(f) = open(b, lim) {
        for i in 0..f.num_pages().min(8) {
            if let Ok(p) = f.page(i) {
                let _ = p.rows(&AtomicBool::new(false), |_, _| ControlFlow::Continue(()));
            }
        }
    }
}

#[test]
fn fuzz_seeds() {
    let mut seeds = Vec::new();
    for comp in [1, 5, 7, 8, 32773] {
        for tiles in [false, true] {
            let mut p = Page {
                w: 33,
                h: 21,
                spp: 3,
                bps: 8,
                photometric: if comp == 7 { 6 } else { 2 }, // jpeg-rs writes YCbCr
                compression: comp,
                rows_per_strip: 5,
                ..Default::default()
            };
            if tiles {
                (p.rows_per_strip, p.tile_w, p.tile_l) = (0, 16, 16);
            }
            seeds.push(
                File {
                    pages: vec![p.clone(), p],
                    ..Default::default()
                }
                .bytes(),
            );
        }
    }
    for e in fs::read_dir(testdata(&["fuzz", "FuzzTIFF"])).unwrap() {
        seeds.push(unquote_seed(
            &fs::read_to_string(e.unwrap().path()).unwrap(),
        ));
    }
    let lim = Limits {
        max_ifds: 64,
        max_pixels: 1 << 22,
        max_band_bytes: 1 << 20,
        max_alloc: 96 << 20,
        ..Default::default()
    };
    let mut x = 0x2545_f491_4f6c_dd1du64;
    let mut rnd = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for s in &seeds {
        run_all(s, &lim);
        for k in 0..150 {
            let mut b = s.clone();
            match k % 3 {
                0 => b.truncate((rnd() % (b.len() as u64 + 1)) as usize),
                _ => {
                    for _ in 0..1 + k % 4 {
                        let i = (rnd() % b.len() as u64) as usize;
                        b[i] ^= 1 << (rnd() % 8);
                    }
                }
            }
            run_all(&b, &lim);
        }
    }
}

// CCITT's two row buffers are charged against `max_alloc` (the strip's own row is 128 KiB).
#[test]
fn ccitt_row_buffers_charged() {
    let p = Page {
        w: 1 << 20,
        h: 1,
        spp: 1,
        bps: 1,
        compression: 4,
        chunks: Some(vec![vec![0u8; 4]]),
        ..Default::default()
    };
    let b = File {
        pages: vec![p],
        ..Default::default()
    }
    .bytes();
    let lim = Limits {
        max_alloc: 200_000,
        ..Default::default()
    };
    assert!(
        matches!(decode(&b, &lim, 0), Err(Error::Limit { name: "alloc", .. })),
        "ccitt buffers"
    );
}

/// Sets `flag` once `after` reads have been made: a cancel that arrives while a row is decoded.
struct Trip<'a> {
    b: &'a [u8],
    flag: &'a AtomicBool,
    after: AtomicUsize,
}

impl tiff::ReadAt for Trip<'_> {
    fn read_at(&self, buf: &mut [u8], off: u64) -> std::io::Result<usize> {
        // ReadAt is Sync: take one read off the count atomically; at 0 the flag is set.
        let left = self
            .after
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1));
        if left.is_err() {
            self.flag.store(true, Ordering::Relaxed);
        }
        <[u8] as tiff::ReadAt>::read_at(self.b, buf, off)
    }
}

/// A strip whose bytes produce no output must still be cancellable mid-row.
fn cancelled_mid_row(comp: usize, bps: usize, chunk: Vec<u8>) {
    let p = Page {
        w: 8,
        h: 1,
        spp: 1,
        bps,
        compression: comp,
        chunks: Some(vec![chunk]),
        ..Default::default()
    };
    let b = File {
        pages: vec![p],
        ..Default::default()
    }
    .bytes();
    let flag = AtomicBool::new(false);
    let ra = Trip {
        b: &b,
        flag: &flag,
        after: AtomicUsize::new(100),
    };
    let f = tiff::File::open(ra, b.len() as u64, &Limits::default()).unwrap();
    let p = f.page(0).unwrap();
    let r = p.rows(&flag, |_, _| ControlFlow::Continue(()));
    assert_eq!(r, Err(Error::Cancelled), "comp {comp}");
}

#[test]
fn cancel_inside_outputless_streams() {
    // PackBits: 0x80 is a no-op header.
    cancelled_mid_row(32773, 8, vec![0x80; 4 << 20]);
    // LZW: clear codes (9 bits of 256).
    let lzw: Vec<u8> = [0x80, 0x40, 0x20, 0x10, 0x08, 0x04, 0x02, 0x01, 0x00]
        .iter()
        .copied()
        .cycle()
        .take(4 << 20)
        .collect();
    cancelled_mid_row(5, 8, lzw);
    // CCITT G4: horizontal mode with a white and a black run of length 0 never advances.
    let mut bits = String::new();
    while bits.len() < 8 << 20 {
        bits.push_str("001001101010000110111");
    }
    let g4: Vec<u8> = bits
        .as_bytes()
        .chunks(8)
        .filter(|c| c.len() == 8)
        .map(|c| c.iter().fold(0u8, |a, &x| a << 1 | (x - b'0')))
        .collect();
    cancelled_mid_row(4, 1, g4);
}
