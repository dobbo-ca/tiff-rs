//! `Page::raw_jpeg_into`: the stream it returns, decoded with jpeg-rs, is byte for byte what
//! `Page::decode_chunk_into` gives for the chunk; and its errors.

mod common;

use common::{File, Page, marked, pattern, ycc_jpeg};
use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use tiff::{Decoder, Error, Limits};

fn data(dir: &str, name: &str) -> Vec<u8> {
    fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data")
            .join(dir)
            .join(name),
    )
    .unwrap()
}

/// What tiff-rs's JPEG path delivers for a decoded frame: grey, or RGB with the alpha dropped,
/// rows packed.
fn pixels(dec: &mut jpeg::Decoder, stream: &[u8], spp: usize) -> Vec<u8> {
    let o = jpeg::DecodeOptions {
        format: if spp == 1 {
            jpeg::Format::Gray
        } else {
            jpeg::Format::Rgba
        },
        ..Default::default()
    };
    let lim = jpeg::Limits {
        max_threads: 1,
        ..Default::default()
    };
    let d = dec
        .decode(stream, &o, &lim, &AtomicBool::new(false))
        .unwrap_or_else(|e| panic!("jpeg decode: {e:?}"));
    assert!(d.error.is_none(), "jpeg decode: {:?}", d.error);
    let mut out = Vec::new();
    match &d.image {
        jpeg::Image::Gray(g) => {
            let w = g.rect.width() as usize;
            for y in 0..g.rect.height() {
                let o = g.pix_offset(g.rect.min_x, g.rect.min_y + y);
                out.extend_from_slice(&g.pix[o..o + w]);
            }
        }
        jpeg::Image::Rgba(m) => {
            let w = m.rect.width() as usize;
            for y in 0..m.rect.height() {
                let o = m.pix_offset(m.rect.min_x, m.rect.min_y + y);
                for x in 0..w {
                    out.extend_from_slice(&m.pix[o + 4 * x..o + 4 * x + 3]);
                }
            }
        }
        _ => panic!("unexpected image type"),
    }
    out
}

/// Every chunk of every JPEG page (1 or 3 samples) of `b`: the raw stream decodes to the chunk's
/// pixels. Returns how many chunks it compared.
fn compare(name: &str, b: &[u8]) -> usize {
    let f = tiff::File::open(b, b.len() as u64, &Limits::default()).unwrap();
    let cancel = AtomicBool::new(false);
    let (mut tdec, mut jdec) = (Decoder::new(), jpeg::Decoder::new());
    let (mut want, mut raw) = (Vec::new(), Vec::new());
    let mut n = 0;
    for pi in 0..f.num_pages() {
        let page = f.page(pi).unwrap();
        let info = page.info();
        if info.compression != 7 || !matches!(info.samples_per_pixel, 1 | 3) {
            continue;
        }
        for i in 0..page.layout().chunks {
            page.decode_chunk_into(i, &mut tdec, &mut want, &cancel)
                .unwrap();
            page.raw_jpeg_into(i, &mut raw).unwrap();
            let got = pixels(&mut jdec, &raw, info.samples_per_pixel);
            assert!(got == want, "{name} page {pi} chunk {i}: pixels differ");
            n += 1;
        }
    }
    n
}

#[test]
fn fixture_files() {
    let mut files = 0;
    for dir in ["golden", "synthetic"] {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data")
            .join(dir);
        for e in fs::read_dir(root).unwrap() {
            let path = e.unwrap().path();
            if path.extension().is_some_and(|x| x == "tif") {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                if compare(&name, &fs::read(&path).unwrap()) > 0 {
                    files += 1;
                }
            }
        }
    }
    // grey_jpeg_strip, rgb_jpeg_strip, rgb_jpeg_tile: all with JPEGTables, the RGB ones YCbCr
    // with subsampling.
    assert!(files >= 3, "only {files} JPEG files compared");
}

/// A one-chunk JPEG TIFF of the given tables and chunk; three samples are YCbCr, as the streams
/// here are.
fn one_chunk(tables: Option<Vec<u8>>, chunk: Vec<u8>, w: usize, h: usize, spp: usize) -> Vec<u8> {
    let photometric = if spp == 1 { 1 } else { 6 };
    one_chunk_as(tables, chunk, (w, h, spp), photometric)
}

/// [`one_chunk`] with the given photometric.
fn one_chunk_as(
    tables: Option<Vec<u8>>,
    chunk: Vec<u8>,
    (w, h, spp): (usize, usize, usize),
    photometric: usize,
) -> Vec<u8> {
    File {
        pages: vec![Page {
            w,
            h,
            spp,
            bps: 8,
            photometric,
            compression: 7,
            rows_per_strip: h,
            chunks: Some(vec![chunk]),
            jpeg_tables: tables,
            ..Default::default()
        }],
        ..Default::default()
    }
    .bytes()
}

/// The DQT and DHT segments before the first SOS of `j` moved to a tables stream (with `extra`
/// segments after its SOI), and the chunk that is left.
fn split(j: &[u8], extra: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut tables = vec![0xff, 0xd8];
    tables.extend_from_slice(extra);
    let mut tile = vec![0xff, 0xd8];
    let mut i = 2;
    while j[i + 1] != 0xda {
        assert_eq!(j[i], 0xff);
        let len = usize::from(u16::from_be_bytes([j[i + 2], j[i + 3]])) + 2;
        let dst = if matches!(j[i + 1], 0xdb | 0xc4) {
            &mut tables
        } else {
            &mut tile
        };
        dst.extend_from_slice(&j[i..i + len]);
        i += len;
    }
    tile.extend_from_slice(&j[i..]);
    tables.extend_from_slice(&[0xff, 0xd9]);
    (tables, tile)
}

/// Segments that jpeg-rs forgets at the chunk's SOI: Adobe with transform 0 (RGB), a restart
/// interval of 3 (the entropy data has no RSTn), JFIF and a comment.
const EXTRA: &[u8] = &[
    0xff, 0xee, 0x00, 0x0e, b'A', b'd', b'o', b'b', b'e', 0, 100, 0, 0, 0, 0, 0, //
    0xff, 0xdd, 0x00, 0x04, 0x00, 0x03, //
    0xff, 0xe0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0, //
    0xff, 0xfe, 0x00, 0x05, b'a', b'b', b'c',
];

const JPEGS: [&str; 5] = [
    "jpeg_baseline",
    "jpeg_noninterleaved",
    "jpeg_progressive_420",
    "jpeg_progressive_444",
    "jpeg_progressive_fill",
];

#[test]
fn streams_without_tables_come_back_as_stored() {
    for name in JPEGS {
        let j = data("synthetic", &format!("{name}.jpg"));
        let c = jpeg::decode_config(&j, None, &jpeg::Limits::default()).unwrap();
        let b = one_chunk(None, j.clone(), c.width, c.height, c.components);
        assert_eq!(compare(name, &b), 1);
        let f = tiff::File::open(b.as_slice(), b.len() as u64, &Limits::default()).unwrap();
        let mut out = vec![9u8; 3];
        f.page(0).unwrap().raw_jpeg_into(0, &mut out).unwrap();
        assert!(out == j, "{name}: not as stored");
    }
}

#[test]
fn merged_tables_decode_as_the_pair() {
    for name in &JPEGS[..4] {
        let j = data("synthetic", &format!("{name}.jpg"));
        let c = jpeg::decode_config(&j, None, &jpeg::Limits::default()).unwrap();
        for extra in [&[0u8; 0][..], EXTRA] {
            let (tables, tile) = split(&j, extra);
            let b = one_chunk(Some(tables), tile, c.width, c.height, c.components);
            assert_eq!(compare(name, &b), 1);
            let f = tiff::File::open(b.as_slice(), b.len() as u64, &Limits::default()).unwrap();
            let mut out = Vec::new();
            f.page(0).unwrap().raw_jpeg_into(0, &mut out).unwrap();
            // Only the tables' DQT and DHT are kept: no Adobe, no DRI (stuffed entropy data has
            // no 0xff 0xdd).
            assert!(
                !out.windows(2)
                    .any(|w| w == [0xff, 0xee] || w == [0xff, 0xdd])
            );
        }
    }
}

/// What the walker of the tables meets that is not a table: fill bytes, TEM, RST0, an APPn with
/// `FF 00` in its body, APPn and DNL with a length under 2.
const WALK: &[u8] = &[
    0xff, 0xff, 0xff, 0x01, //
    0xff, 0xd0, 0x00, 0x00, //
    0xff, 0xe1, 0x00, 0x06, 0xff, 0x00, 0xff, 0x00, //
    0xff, 0xe1, 0x00, 0x00, //
    0xff, 0xdc, 0x00, 0x01,
];

#[test]
fn table_walker_branches_match_the_pair() {
    for name in &JPEGS[..4] {
        let j = data("synthetic", &format!("{name}.jpg"));
        let c = jpeg::decode_config(&j, None, &jpeg::Limits::default()).unwrap();
        let (mut tables, tile) = split(&j, WALK);
        // Bytes after the tables' EOI are not read.
        tables.extend_from_slice(&[1, 2, 3, 0xff, 0xc0, 0xff]);
        let b = one_chunk(Some(tables), tile, c.width, c.height, c.components);
        assert_eq!(compare(name, &b), 1);
    }
}

// A tables stream that ends inside an APPn, COM or DNL segment, as jpeg-rs lets it, and a DAC that
// is checked but not copied.
#[test]
fn cut_and_arithmetic_tables() {
    let j = data("synthetic", "jpeg_baseline.jpg");
    let c = jpeg::decode_config(&j, None, &jpeg::Limits::default()).unwrap();
    // The tables' DQT and DHT, then a cut COM, with no EOI.
    let (mut tables, tile) = split(&j, &[]);
    tables.truncate(tables.len() - 2);
    tables.extend_from_slice(&[0xff, 0xfe, 0x00, 0x10, 0x61]);
    let b = one_chunk(Some(tables), tile, c.width, c.height, c.components);
    assert_eq!(compare("cut COM", &b), 1);
    // A good DAC is not kept; a bad one (index 32) is refused.
    for (dac, ok) in [([0x00u8, 0x11], true), ([0x20, 0x11], false)] {
        let mut extra = vec![0xff, 0xcc, 0x00, 0x04];
        extra.extend_from_slice(&dac);
        let (tables, tile) = split(&j, &extra);
        let b = one_chunk(Some(tables), tile, c.width, c.height, c.components);
        let f = tiff::File::open(b.as_slice(), b.len() as u64, &Limits::default()).unwrap();
        let mut out = Vec::new();
        let r = f.page(0).unwrap().raw_jpeg_into(0, &mut out);
        if ok {
            r.unwrap();
            assert!(!out.windows(2).any(|w| w == [0xff, 0xcc]));
        } else {
            assert!(matches!(r, Err(Error::Corrupt(_))), "{r:?}");
            assert!(out.is_empty());
        }
    }
}

// A frame that is not the chunk's: another size, or another component count. `decode_chunk_into`
// refuses it, and so does `raw_jpeg_into`.
#[test]
fn frame_that_is_not_the_chunk() {
    let j = data("synthetic", "jpeg_baseline.jpg");
    let c = jpeg::decode_config(&j, None, &jpeg::Limits::default()).unwrap();
    let other = if c.components == 1 { 3 } else { 1 };
    for (w, h, spp) in [
        (c.width + 8, c.height, c.components),
        (c.width, c.height + 8, c.components),
        (c.width, c.height, other),
    ] {
        let b = one_chunk(None, j.clone(), w, h, spp);
        let f = tiff::File::open(b.as_slice(), b.len() as u64, &Limits::default()).unwrap();
        let page = f.page(0).unwrap();
        let mut out = vec![1u8; 3];
        let r = page.raw_jpeg_into(0, &mut out);
        assert!(
            matches!(r, Err(Error::Corrupt(_))),
            "{w}x{h} spp {spp}: {r:?}"
        );
        assert!(out.is_empty());
        let r = page.decode_chunk_into(0, &mut Decoder::new(), &mut out, &AtomicBool::new(false));
        assert!(
            matches!(r, Err(Error::Corrupt(_))),
            "{w}x{h} spp {spp}: {r:?}"
        );
    }
}

#[test]
fn errors() {
    let lim = Limits::default();
    let j = data("synthetic", "jpeg_baseline.jpg");
    let c = jpeg::decode_config(&j, None, &jpeg::Limits::default()).unwrap();
    let (tables, tile) = split(&j, &[]);
    let junk = vec![7u8; 5];

    // Not JPEG, and four samples.
    let grey = File {
        pages: vec![Page {
            w: 4,
            h: 4,
            spp: 1,
            bps: 8,
            photometric: 1,
            ..Default::default()
        }],
        ..Default::default()
    }
    .bytes();
    let f = tiff::File::open(grey.as_slice(), grey.len() as u64, &lim).unwrap();
    let mut out = junk.clone();
    let r = f.page(0).unwrap().raw_jpeg_into(0, &mut out);
    assert!(matches!(r, Err(Error::Unsupported(_))), "{r:?}");
    assert!(out.is_empty());
    let cmyk = data("synthetic", "cmyk_jpeg_bs.tif");
    let f = tiff::File::open(cmyk.as_slice(), cmyk.len() as u64, &lim).unwrap();
    let r = f.page(0).unwrap().raw_jpeg_into(0, &mut out);
    assert!(matches!(r, Err(Error::Unsupported(_))), "{r:?}");

    // An index past the last chunk.
    let tiles = data("golden", "rgb_jpeg_tile.tif");
    let f = tiff::File::open(tiles.as_slice(), tiles.len() as u64, &lim).unwrap();
    let p = f.page(0).unwrap();
    out = junk.clone();
    let r = p.raw_jpeg_into(p.layout().chunks, &mut out);
    assert!(matches!(r, Err(Error::Unsupported(_))), "{r:?}");
    assert!(out.is_empty());
    p.raw_jpeg_into(p.layout().chunks - 1, &mut out).unwrap();

    // Bad tables.
    let bad: [Vec<u8>; 5] = [
        vec![0, 1, 2, 3],
        vec![0xff],
        vec![0xff, 0xd8, 0xff, 0xdb, 0x00, 0xff, 1, 2, 3],
        vec![0xff, 0xd8, 0xff, 0xc0, 0x00, 0x02, 0xff, 0xd9],
        vec![0xff, 0xd8, 0xff, 0xdd, 0x00, 0x05, 0, 3, 0, 0xff, 0xd9],
    ];
    for t in bad {
        let b = one_chunk(
            Some(t.clone()),
            tile.clone(),
            c.width,
            c.height,
            c.components,
        );
        let f = tiff::File::open(b.as_slice(), b.len() as u64, &lim).unwrap();
        out = junk.clone();
        let r = f.page(0).unwrap().raw_jpeg_into(0, &mut out);
        assert!(matches!(r, Err(Error::Corrupt(_))), "{t:?}: {r:?}");
        assert!(out.is_empty());
    }

    // A chunk that is cut short or does not start with SOI, with and without tables.
    for t in [None, Some(tables)] {
        for chunk in [vec![0xff], vec![0x00, 0x01, 0x02, 0x03], tile[2..].to_vec()] {
            let b = one_chunk(t.clone(), chunk.clone(), c.width, c.height, c.components);
            let f = tiff::File::open(b.as_slice(), b.len() as u64, &lim).unwrap();
            let r = f.page(0).unwrap().raw_jpeg_into(0, &mut out);
            assert!(matches!(r, Err(Error::Corrupt(_))), "{r:?}");
            assert!(out.is_empty());
        }
    }

    // The size of the stream is held to max_alloc, before anything is allocated.
    let b = one_chunk(None, j, c.width, c.height, c.components);
    let small = Limits {
        max_alloc: 100,
        ..lim
    };
    let f = tiff::File::open(b.as_slice(), b.len() as u64, &small).unwrap();
    let r = f.page(0).unwrap().raw_jpeg_into(0, &mut out);
    assert!(
        matches!(r, Err(Error::Limit { name: "alloc", .. })),
        "{r:?}"
    );
    assert!(out.is_empty());
}

/// An Adobe APP14 segment with transform 0: RGB.
const APP14_RGB: [u8; 16] = [
    0xff, 0xee, 0, 14, b'A', b'd', b'o', b'b', b'e', 0, 100, 0, 0, 0, 0, 0,
];

/// A JFIF APP0 or Adobe APP14 segment, by its marker and body.
fn colour_segment(m: u8, d: &[u8]) -> bool {
    (m == 0xe0 && d.starts_with(b"JFIF\0")) || (m == 0xee && d.starts_with(b"Adobe"))
}

/// The stream `s` (no fill bytes) without the JFIF and Adobe segments before its first SOS.
fn uncoloured(s: &[u8]) -> Vec<u8> {
    let mut v = s[..2].to_vec();
    let mut i = 2;
    while s[i + 1] != 0xda {
        let e = i + 2 + usize::from(u16::from_be_bytes([s[i + 2], s[i + 3]]));
        if !colour_segment(s[i + 1], &s[i + 4..e]) {
            v.extend_from_slice(&s[i..e]);
        }
        i = e;
    }
    v.extend_from_slice(&s[i..]);
    v
}

// A page with Photometric RGB holds RGB samples that the decode reads as stored; its raw stream
// says so itself: the JFIF and Adobe segments are dropped and an Adobe APP14 transform 0 follows
// the SOI, so jpeg-rs's default reading of the markers gives what the decode gives. Every other
// byte is the YCbCr page's stream, which is the chunk (and tables) unchanged.
#[test]
fn rgb_page_stream_says_rgb() {
    let (w, h) = (48, 32);
    let j = ycc_jpeg(&pattern(w, h, 3, 8), w, h, true);
    for (name, chunk) in marked(&j) {
        let dropped = chunk.len() - uncoloured(&chunk).len();
        for tables in [false, true] {
            let at = format!("{name} tables {tables}");
            let (t, c) = if tables {
                let (t, c) = split(&chunk, &[]);
                (Some(t), c)
            } else {
                (None, chunk.clone())
            };
            // The tables' DQT and DHT, without their SOI and EOI.
            let kept = t.as_ref().map_or(0, |t| t.len() - 4);
            let mut out = [Vec::new(), Vec::new()];
            for (o, photometric) in out.iter_mut().zip([2, 6]) {
                let b = one_chunk_as(t.clone(), c.clone(), (w, h, 3), photometric);
                assert_eq!(compare(&at, &b), 1, "{at} photometric {photometric}");
                let f = tiff::File::open(b.as_slice(), b.len() as u64, &Limits::default()).unwrap();
                f.page(0).unwrap().raw_jpeg_into(0, o).unwrap();
            }
            let [rgb, ycc] = out;
            assert_eq!(ycc.len(), c.len() + kept, "{at}");
            if !tables {
                assert!(ycc == chunk, "{at}: ycbcr stream not as stored");
            }
            assert_eq!(rgb.len(), c.len() + kept + 16 - dropped, "{at}");
            assert_eq!(rgb[..2], [0xff, 0xd8], "{at}");
            assert_eq!(rgb[2..18], APP14_RGB, "{at}");
            let rest = uncoloured(&rgb);
            assert_eq!(rest.len(), rgb.len() - 16, "{at}: a colour segment is left");
            assert!(rest == uncoloured(&ycc), "{at}: other bytes differ");
        }
    }
}
