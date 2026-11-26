//! Ports of tiff-go's entries_test.go and skip_test.go: Orientation and XMP entries, resolution
//! and the tags skipped for a count or offset outside the file.

mod common;

use common::decode;
use tiff::{Error, Limits, TagEntry};

fn le16(b: &mut Vec<u8>, v: u16) {
    b.extend(v.to_le_bytes());
}

fn le32(b: &mut Vec<u8>, v: u32) {
    b.extend(v.to_le_bytes());
}

fn le64(b: &mut Vec<u8>, v: u64) {
    b.extend(v.to_le_bytes());
}

/// A 1x1 8-bit grey TIFF, classic or BigTIFF, plus one extra tag whose count and value field
/// are written as given.
fn oob_tiff(big: bool, tag: u16, typ: u16, count: u64, val: u64, more: &[[u64; 4]]) -> Vec<u8> {
    let mut b = if big {
        let mut v = b"II+\x00\x08\x00\x00\x00".to_vec();
        v.extend([0u8; 8]);
        v
    } else {
        b"II*\x00\x00\x00\x00\x00".to_vec()
    };
    let data = b.len() as u64;
    b.push(7);
    let at = b.len() as u64;
    if big {
        b[8..16].copy_from_slice(&at.to_le_bytes());
    } else {
        b[4..8].copy_from_slice(&(at as u32).to_le_bytes());
    }
    let mut tags = vec![
        [256, 4, 1, 1],
        [257, 4, 1, 1],
        [258, 3, 1, 8],
        [262, 3, 1, 1],
        [273, 4, 1, data],
        [278, 4, 1, 1],
        [279, 4, 1, 1],
        [u64::from(tag), u64::from(typ), count, val],
    ];
    tags.extend_from_slice(more);
    if big {
        le64(&mut b, tags.len() as u64);
    } else {
        le16(&mut b, tags.len() as u16);
    }
    for t in &tags {
        le16(&mut b, t[0] as u16);
        le16(&mut b, t[1] as u16);
        if big {
            le64(&mut b, t[2]);
            le64(&mut b, t[3]);
        } else {
            le32(&mut b, t[2] as u32);
            le32(&mut b, t[3] as u32);
        }
    }
    if big {
        le64(&mut b, 0);
    } else {
        le32(&mut b, 0);
    }
    b
}

fn page0(b: &[u8], lim: &Limits) -> (tiff::Info, Vec<TagEntry>, Vec<TagEntry>) {
    let f = tiff::File::open(b, b.len() as u64, lim).unwrap();
    let p = f.page(0).unwrap();
    (p.info(), p.orientations.clone(), p.xmps.clone())
}

fn entry(typ: u16, count: u64) -> TagEntry {
    TagEntry {
        typ,
        count,
        ..Default::default()
    }
}

#[test]
fn orientation_entries() {
    for big in [false, true] {
        let b = oob_tiff(
            big,
            274,
            3,
            1,
            6,
            &[[274, 3, 1, 8], [274, 12, 1, 0], [274, 3, 1 << 30, 0]],
        );
        let (info, or, _) = page0(&b, &Limits::default());
        let dup = |e: TagEntry| TagEntry {
            duplicate: true,
            ..e
        };
        let want = [
            dup(TagEntry {
                value: 6,
                ..entry(3, 1)
            }),
            dup(TagEntry {
                value: 8,
                ..entry(3, 1)
            }),
            dup(TagEntry {
                bad_type: true,
                ..entry(12, 1)
            }),
            dup(TagEntry {
                out_of_range: true,
                ..entry(3, 1 << 30)
            }),
        ];
        assert_eq!(or, want, "big={big}");
        assert_eq!(info.skipped, [274]);
    }
}

#[test]
fn xmp_entries() {
    for big in [false, true] {
        // inline (4 bytes), out of range, wrong type, over the cap
        let b = oob_tiff(
            big,
            700,
            1,
            4,
            0x3433_3231,
            &[[700, 1, 1 << 30, 0], [700, 3, 1, 1], [700, 1, 9, 0]],
        );
        let lim = Limits {
            max_metadata_bytes: 8,
            ..Default::default()
        };
        let (_, _, x) = page0(&b, &lim);
        assert!(
            x.len() == 4
                && x[0].data.as_deref() == Some(b"1234".as_slice())
                && !x[0].out_of_range
                && x[0].duplicate
                && x[1].out_of_range
                && x[1].data.is_none()
                && x[2].bad_type
                && x[2].data.is_none()
                && x[3].over_limit
                && x[3].data.is_none(),
            "big={big} {x:?}"
        );
    }
    let (_, or, x) = page0(&oob_tiff(false, 700, 7, 2, 0x4241, &[]), &Limits::default());
    assert!(
        x.len() == 1
            && x[0].data.as_deref() == Some(b"AB".as_slice())
            && !x[0].duplicate
            && or.is_empty(),
        "{x:?}"
    );
}

// Counts near the maximum and offsets near 2^63 and 2^64 never wrap: the entry is out of range,
// listed, and skipped for decoding.
#[test]
fn entries_overflow_safe() {
    for typ in [1u16, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 16, 17, 18] {
        for big in [false, true] {
            let mut cs = vec![(1u64 << 32) - 1, (1 << 32) - 2];
            let mut os = vec![(1u64 << 32) - 1, (1 << 32) - 8, 1 << 20];
            if big {
                cs.extend([1 << 63, (1 << 63) - 1, u64::MAX, u64::MAX - 1]);
                os.extend([(1 << 63) - 8, 1 << 63, u64::MAX - 7, u64::MAX]);
            }
            for &c in &cs {
                for &o in &os {
                    for tag in [274u16, 700] {
                        let (info, or, x) =
                            page0(&oob_tiff(big, tag, typ, c, o, &[]), &Limits::default());
                        let es = if tag == 700 { x } else { or };
                        assert!(
                            es.len() == 1
                                && es[0].out_of_range
                                && es[0].data.is_none()
                                && es[0].value == 0
                                && info.skipped == [tag],
                            "typ {typ} big {big} {c:x} {o:x} tag {tag} {es:?}"
                        );
                    }
                }
            }
        }
    }
}

// Orientation entries list in every tag order, whatever the last entry is.
#[test]
fn orientation_entries_any_order() {
    let bad = [
        [274, 12, 1, 0],
        [274, 8, 1, 0],
        [274, 2, 1, 0],
        [274, 3, 2, 0],
        [274, 3, 0, 0],
    ];
    for big in [false, true] {
        for x in bad {
            for first in [false, true] {
                let b = if first {
                    oob_tiff(big, x[0] as u16, x[1] as u16, x[2], x[3], &[[274, 3, 1, 6]])
                } else {
                    oob_tiff(big, 274, 3, 1, 6, &[x])
                };
                let (_, or, _) = page0(&b, &Limits::default());
                assert_eq!(or.len(), 2, "big={big} {x:?} first={first}: {or:?}");
            }
        }
        let (_, or, _) = page0(&oob_tiff(big, 274, 8, 1, 6, &[]), &Limits::default());
        assert!(or.len() == 1 && or[0].bad_type, "lone sshort: {or:?}");
    }
}

#[test]
fn resolution_entries() {
    let rat = |n: u32, d: u32| [n.to_le_bytes(), d.to_le_bytes()].concat();
    for big in [false, true] {
        // classic stores a RATIONAL at an offset past the IFD, BigTIFF inline
        let build = |x: &[u8], y: &[u8]| {
            let val = |r: &[u8], off: u64| {
                if big {
                    u64::from_le_bytes(r.try_into().unwrap())
                } else {
                    off
                }
            };
            let b = oob_tiff(big, 282, 5, 1, 0, &[[283, 5, 1, 0], [296, 3, 1, 3]]);
            let n = b.len() as u64;
            let mut b = oob_tiff(
                big,
                282,
                5,
                1,
                val(x, n),
                &[[283, 5, 1, val(y, n + 8)], [296, 3, 1, 3]],
            );
            if !big {
                b.extend(x);
                b.extend(y);
            }
            b
        };
        let (got, _, _) = page0(&build(&rat(300, 1), &rat(7, 2)), &Limits::default());
        assert!(
            got.x_resolution == 300.0 && got.y_resolution == 3.5 && got.resolution_unit == 3,
            "big={big} {got:?}"
        );
        let (got, _, _) = page0(&build(&rat(300, 0), &rat(7, 2)), &Limits::default());
        assert!(
            got.x_resolution == 0.0 && got.y_resolution == 3.5,
            "zero denominator big={big} {got:?}"
        );
        let b = oob_tiff(big, 282, 5, 1 << 62, 1 << 62, &[[296, 3, 1 << 30, 0]]);
        let (got, _, _) = page0(&b, &Limits::default());
        assert!(
            got.x_resolution == 0.0 && got.resolution_unit == 2,
            "out of range big={big} {got:?}"
        );
    }
}

#[test]
fn skipped_out_of_range_tags() {
    let types: [(&str, u16); 16] = [
        ("BYTE", 1),
        ("ASCII", 2),
        ("SHORT", 3),
        ("LONG", 4),
        ("RATIONAL", 5),
        ("SBYTE", 6),
        ("UNDEFINED", 7),
        ("SSHORT", 8),
        ("SLONG", 9),
        ("SRATIONAL", 10),
        ("FLOAT", 11),
        ("DOUBLE", 12),
        ("IFD", 13),
        ("LONG8", 16),
        ("SLONG8", 17),
        ("IFD8", 18),
    ];
    for (name, typ) in types {
        for big in [false, true] {
            let vw: u64 = if big { 8 } else { 4 };
            let sz: u64 = match typ {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 | 13 => 4,
                _ => 8,
            };
            for (kind, cv) in [
                ("count", [1u64 << 30, 0]),
                ("offset", [vw / sz + 1, 1 << 20]),
            ] {
                let b = oob_tiff(big, 700, typ, cv[0], cv[1], &[]);
                let (got, _) = decode(&b, &Limits::default(), 0)
                    .unwrap_or_else(|e| panic!("{name}/{big}/{kind}: {e}"));
                assert_eq!(got, [7], "{name}/{big}/{kind}");
                let (info, _, _) = page0(&b, &Limits::default());
                assert_eq!(info.skipped, [700], "{name}/{big}/{kind}");
            }
        }
    }
}

#[test]
fn skipped_orientation_reads_as_one() {
    for big in [false, true] {
        let (info, _, _) = page0(&oob_tiff(big, 274, 3, 1 << 30, 0, &[]), &Limits::default());
        assert!(
            info.orientation == 1 && info.skipped == [274],
            "big={big} {info:?}"
        );
    }
}

#[test]
fn out_of_range_needed_tag_stays_corrupt() {
    for big in [false, true] {
        for c in [[1u64 << 30, 0], [100, 1 << 20]] {
            for tag in [338u16, 339] {
                // ExtraSamples, SampleFormat
                let b = oob_tiff(big, tag, 3, c[0], c[1], &[]);
                let r = decode(&b, &Limits::default(), 0);
                assert!(
                    matches!(r, Err(Error::Corrupt(_))),
                    "big={big} tag {tag} {c:?}: {:?}",
                    r.err()
                );
            }
        }
    }
}

// Tags the page does not read are skipped even when decode-relevant in general: ColorMap without
// Palette, JPEGTables without JPEG, tile tables on a strip file, CCITT options on an
// uncompressed file, FillOrder.
#[test]
fn skipped_tags_this_page_does_not_read() {
    for big in [false, true] {
        for tag in [320u16, 347, 292, 293, 324, 325, 266] {
            for cv in [[1u64 << 30, 0], [3, 1 << 20]] {
                let b = oob_tiff(big, tag, 4, cv[0], cv[1], &[]);
                let r = decode(&b, &Limits::default(), 0);
                assert!(
                    matches!(&r, Ok((g, _)) if g == &[7]),
                    "big={big} tag {tag} {cv:?}: {:?}",
                    r.err()
                );
            }
        }
    }
}

#[test]
fn duplicate_orientation_last_wins() {
    // a valid Orientation=6 followed by an out-of-range one reads as 1 and is skipped.
    let b = oob_tiff(false, 274, 3, 1, 6, &[[274, 3, 1 << 30, 0]]);
    let (info, _, _) = page0(&b, &Limits::default());
    assert!(info.orientation == 1 && info.skipped == [274], "{info:?}");
}

#[test]
fn skipped_once_and_tiled_ignores_rows_per_strip() {
    let b = oob_tiff(false, 700, 3, 1 << 30, 0, &[[700, 3, 1 << 30, 0]]);
    let (info, _, _) = page0(&b, &Limits::default());
    assert_eq!(info.skipped, [700]);
    // tile tags make 278 unread: libtiff-style skip, here reported
    let b = oob_tiff(
        false,
        278,
        4,
        1 << 30,
        0,
        &[
            [322, 4, 1, 1],
            [323, 4, 1, 1],
            [324, 4, 1, 8],
            [325, 4, 1, 1],
        ],
    );
    let (info, _, _) = page0(&b, &Limits::default());
    assert!(info.skipped.contains(&278), "{info:?}");
}

// An out-of-range JPEGTables on a JPEG page is corrupt, not a limit error.
#[test]
fn out_of_range_jpeg_tables_is_corrupt() {
    for big in [false, true] {
        let b = oob_tiff(big, 347, 7, 1 << 30, 0, &[[259, 3, 1, 7]]);
        let r = decode(&b, &Limits::default(), 0);
        assert!(
            matches!(r, Err(Error::Corrupt(_))),
            "big={big} {:?}",
            r.err()
        );
    }
}

#[test]
fn bad_entry_then_valid_duplicate_reads_valid() {
    let b = oob_tiff(false, 274, 3, 1 << 30, 0, &[[274, 3, 1, 6]]);
    let (info, _, _) = page0(&b, &Limits::default());
    assert!(info.orientation == 6 && info.skipped.is_empty(), "{info:?}");
}

// JPEGTables is charged at count * type size: a DOUBLE array of 1 Mi values is 8 MiB.
#[test]
fn jpeg_tables_charged_by_type_size() {
    let mut b = oob_tiff(false, 347, 12, 1 << 20, 8, &[[259, 3, 1, 7]]);
    b.resize(9 << 20, 0);
    let lim = Limits {
        max_alloc: 2 << 20,
        ..Default::default()
    };
    let f = tiff::File::open(&b[..], b.len() as u64, &lim).unwrap();
    assert!(matches!(f.page(0), Err(Error::Limit { name: "alloc", .. })));
}
