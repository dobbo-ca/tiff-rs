//! Raw JPEG chunks: a strip or tile's compressed bytes as a standalone JPEG stream, no decode.
//!
//! tiff-rs decodes a chunk with `jpeg::decode(chunk, tables)`. jpeg-rs reads the tables stream
//! first and then the chunk, and each SOI resets the state that APPn, COM and DRI segments set
//! (JFIF, Adobe transform, restart interval). So of the tables stream only the quantization and
//! Huffman segments (DQT, DHT) reach the decode. The merge keeps exactly those, and drops every
//! other segment of the tables, so jpeg-rs sees the same colour markers and restart interval in
//! the merged stream as it does in the pair. A DAC segment is checked as jpeg-rs checks it but
//! not copied: libjpeg resets the arithmetic conditioning at the chunk's SOI, and tiff-rs does not
//! decode arithmetic frames. A DQT or DHT in the chunk redefines the table of the same id in both
//! cases, because it comes later. A tables stream that ends inside an APPn, COM or DNL segment, or
//! a DRI of length 4, ends there, as in jpeg-rs.
//!
//! A page with PhotometricInterpretation RGB holds RGB samples that tiff-rs decodes with no colour
//! transform (`jpeg::ColorModel::Rgb`), but the stream itself may say nothing (Aperio SVS: no JFIF,
//! no Adobe segment, ids 0, 1, 2), so every decoder that sees the stream alone reads YCbCr. The
//! standalone stream of such a page says RGB itself: the JFIF APP0 and Adobe APP14 segments before
//! the first scan are dropped (JFIF would win over Adobe, and the last Adobe segment wins), and an
//! Adobe APP14 segment with transform 0 follows the SOI, as opentile does for the same tiles.

use crate::Limits;
use crate::codec::read_err;
use crate::error::{Error, corrupt, limit, unsupported};
use crate::ifd::Container;
use crate::page::{JPEG, Page, RGB};
use crate::readat::ReadAt;
use std::io::Read;

const SOI: [u8; 2] = [0xff, 0xd8];

/// An Adobe APP14 segment with transform 0: the three samples are RGB, no colour transform.
const APP14_RGB: [u8; 16] = [
    0xff, 0xee, 0, 14, b'A', b'd', b'o', b'b', b'e', 0, 100, 0, 0, 0, 0, 0,
];

/// Makes the stream `out` (SOI first) say RGB to any decoder: drops each JFIF APP0 and Adobe
/// APP14 segment before the first scan, found as jpeg-rs finds them (bytes between segments
/// skipped, RSTn and TEM without a length, the identifier read past a short segment's end), then
/// puts [`APP14_RGB`] after the SOI. `out` must have spare capacity for the 16 bytes; nothing is
/// allocated.
fn mark_rgb(out: &mut Vec<u8>) {
    let mut i = 2usize;
    loop {
        while i < out.len() && out[i] != 0xff {
            i += 1;
        }
        while i < out.len() && out[i] == 0xff {
            i += 1;
        }
        let Some(&m) = out.get(i) else { break };
        let s = i - 1;
        i += 1;
        if m == 0 || m == 0x01 || (0xd0..=0xd7).contains(&m) {
            continue;
        }
        if m == 0xd8 || m == 0xd9 || m == 0xda || i + 2 > out.len() {
            break;
        }
        // jpeg-rs reads a length under 2 of an APPn segment as 2.
        let len = usize::from(u16::from_be_bytes([out[i], out[i + 1]])).max(2);
        let d = &out[i + 2..];
        let hit = (m == 0xe0 && d.len() >= 14 && d.starts_with(b"JFIF\0"))
            || (m == 0xee && d.len() >= 12 && d.starts_with(b"Adobe"));
        let e = (i + len).min(out.len());
        if hit {
            out.drain(s..e);
            i = s;
        } else {
            i = e;
        }
    }
    let n = out.len();
    out.resize(n + APP14_RGB.len(), 0);
    out.copy_within(2..n, 2 + APP14_RGB.len());
    out[2..2 + APP14_RGB.len()].copy_from_slice(&APP14_RGB);
}

/// Walks the tables stream `t` as jpeg-rs does (bytes between segments skipped, TEM and RSTn
/// without a length) and validates every segment. Copies the segments that matter to `dst` when
/// it is given. Returns how many bytes those segments take.
fn merge_tables(t: &[u8], mut dst: Option<&mut [u8]>) -> Result<usize, Error> {
    if t.get(..2) != Some(&SOI[..]) {
        return Err(corrupt("jpeg tables do not start with SOI"));
    }
    let (mut i, mut w) = (2usize, 0usize);
    loop {
        while i < t.len() && t[i] != 0xff {
            i += 1;
        }
        while i < t.len() && t[i] == 0xff {
            i += 1;
        }
        let Some(&m) = t.get(i) else { break };
        let s = i - 1; // the 0xff before the marker code
        i += 1;
        if m == 0 || m == 0x01 || (0xd0..=0xd7).contains(&m) {
            continue;
        }
        if m == 0xd9 {
            break;
        }
        if m == 0xd8 {
            return Err(corrupt("jpeg tables hold a second SOI"));
        }
        let skipped = matches!(m, 0xe0..=0xef | 0xfe | 0xdc);
        let Some(lb) = t.get(i..i + 2) else {
            // A cut inside the length bytes ends the tables for these segments, as in jpeg-rs.
            if skipped {
                break;
            }
            return Err(corrupt("jpeg tables segment length"));
        };
        let mut len = usize::from(u16::from_be_bytes([lb[0], lb[1]]));
        // jpeg-rs reads a length under 2 of an APPn, COM or DNL segment as 2.
        if len < 2 && skipped {
            len = 2;
        }
        if len < 2 {
            return Err(corrupt("jpeg tables segment length"));
        }
        let e = i + len;
        if e > t.len() {
            // jpeg-rs ends the tables here for these segments, and fails for the others.
            if skipped || (m == 0xdd && len == 4) {
                break;
            }
            return Err(corrupt("jpeg tables segment past the end"));
        }
        match m {
            0xdb | 0xc4 => {
                if let Some(d) = dst.as_deref_mut() {
                    d[w..w + e - s].copy_from_slice(&t[s..e]);
                }
                w += e - s;
            }
            0xcc => {
                // The rules of jpeg-rs's `read_dac`: pairs, an index under 32, L at most U for DC.
                let (pairs, rest) = t[i + 2..e].as_chunks::<2>();
                let bad =
                    |&[index, val]: &[u8; 2]| index >= 32 || (index < 16 && val & 0x0f > val >> 4);
                if !rest.is_empty() || pairs.iter().any(bad) {
                    return Err(corrupt("jpeg tables hold a bad DAC segment"));
                }
            }
            0xdd => {
                if len != 4 {
                    return Err(corrupt("jpeg tables restart interval length"));
                }
            }
            _ if skipped => {}
            _ => return Err(corrupt("jpeg tables hold an unexpected segment")),
        }
        i = e;
    }
    Ok(w)
}

pub(crate) fn raw_jpeg_into<R: ReadAt>(
    c: &Container<R>,
    p: &Page,
    lim: &Limits,
    i: usize,
    out: &mut Vec<u8>,
) -> Result<(), Error> {
    out.clear();
    let r = run(c, p, lim, i, out);
    if r.is_err() {
        out.clear();
    }
    r
}

fn run<R: ReadAt>(
    c: &Container<R>,
    p: &Page,
    lim: &Limits,
    i: usize,
    out: &mut Vec<u8>,
) -> Result<(), Error> {
    if i as u64 >= p.chunks() {
        return Err(unsupported(format!(
            "chunk {i} of a page with {} chunks",
            p.chunks()
        )));
    }
    if p.compression != JPEG || (p.spp != 1 && p.spp != 3) {
        return Err(unsupported(format!(
            "raw jpeg of compression {} with {} samples",
            p.compression, p.spp
        )));
    }
    let (w, h) = if p.tile_w > 0 {
        (p.tile_w, p.tile_l)
    } else {
        (
            p.width,
            p.rows_per_strip.min(p.height - i * p.rows_per_strip),
        )
    };
    let px = (w as u64).saturating_mul(h as u64);
    if px > lim.max_pixels {
        return Err(limit("pixels", px, lim.max_pixels));
    }
    let (off, len) = c.chunk(&p.offsets, &p.counts, i as u64)?;
    let n = len as u64;
    if n < 2 {
        return Err(corrupt("jpeg chunk shorter than SOI"));
    }
    let t = p.jpeg_tables.as_slice();
    let kept = if t.is_empty() {
        0
    } else {
        merge_tables(t, None)?
    };
    let rgb = p.photometric == RGB && p.spp == 3;
    let mark = if rgb { APP14_RGB.len() as u64 } else { 0 };
    let total = n.saturating_add(kept as u64).saturating_add(mark);
    if total > lim.max_alloc {
        return Err(limit("alloc", total, lim.max_alloc));
    }
    let total = usize::try_from(total).map_err(|_| Error::OutOfMemory)?;
    out.try_reserve_exact(total)
        .map_err(|_| Error::OutOfMemory)?;
    out.resize(total - mark as usize, 0);
    let mut src = c.section(off, len);
    let mut head = [0u8; 2];
    src.read_exact(&mut head).map_err(read_err)?;
    if head != SOI {
        return Err(corrupt("jpeg chunk does not start with SOI"));
    }
    out[..2].copy_from_slice(&head);
    if kept > 0 {
        merge_tables(t, Some(&mut out[2..2 + kept]))?;
    }
    src.read_exact(&mut out[2 + kept..]).map_err(read_err)?;
    if rgb {
        mark_rgb(out);
    }
    // The frame must be the chunk's, as `decode_chunk_into` requires, and jpeg-rs must read the
    // colours as the decode does.
    let cfg = jpeg::decode_config(out, None, &jpeg::Limits::default())
        .map_err(|_| corrupt("jpeg header"))?;
    if cfg.width != w || cfg.height != h || cfg.components != p.spp {
        return Err(corrupt("jpeg frame does not match its chunk"));
    }
    if rgb && !cfg.rgb {
        return Err(corrupt("jpeg colour markers"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(m: u8, data: &[u8]) -> Vec<u8> {
        let mut v = vec![0xff, m];
        v.extend_from_slice(&((data.len() + 2) as u16).to_be_bytes());
        v.extend_from_slice(data);
        v
    }

    fn marked(parts: &[Vec<u8>]) -> Vec<u8> {
        let mut b = SOI.to_vec();
        for p in parts {
            b.extend_from_slice(p);
        }
        b.reserve(APP14_RGB.len());
        mark_rgb(&mut b);
        b
    }

    fn want(parts: &[Vec<u8>]) -> Vec<u8> {
        let mut b = SOI.to_vec();
        b.extend_from_slice(&APP14_RGB);
        for p in parts {
            b.extend_from_slice(p);
        }
        b
    }

    #[test]
    fn mark_rgb_cases() {
        let jfif = seg(0xe0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        let adobe1 = seg(0xee, b"Adobe\0\x64\0\0\0\0\x01");
        let exif = seg(0xe1, b"Exif\0\0");
        let dqt = seg(0xdb, &[0; 65]);
        let sof = seg(
            0xc0,
            &[8, 0, 8, 0, 8, 3, 0, 0x11, 0, 1, 0x11, 0, 2, 0x11, 0],
        );
        let sos = vec![0xff, 0xda, 0, 12, 3, 0, 0, 1, 0, 2, 0, 0, 63, 0, 1, 2];
        // Plain stream: only the APP14 segment is added.
        let plain = [dqt.clone(), sof.clone(), sos.clone()];
        assert_eq!(marked(&plain), want(&plain));
        // JFIF and Adobe transform 1 are dropped; other APPn stay.
        assert_eq!(
            marked(&[
                jfif.clone(),
                exif.clone(),
                adobe1.clone(),
                dqt.clone(),
                sof.clone(),
                sos.clone()
            ]),
            want(&[exif.clone(), dqt.clone(), sof.clone(), sos.clone()])
        );
        // Fill bytes before a segment stay; RSTn has no length.
        let fill = vec![0xff, 0xff];
        let rst = vec![0xff, 0xd0];
        assert_eq!(
            marked(&[
                fill.clone(),
                rst.clone(),
                jfif.clone(),
                sof.clone(),
                sos.clone()
            ]),
            want(&[fill, rst, sof.clone(), sos.clone()])
        );
        // Segments after the first SOS are scan data, left alone.
        let after = [sof.clone(), sos.clone(), jfif.clone()];
        assert_eq!(marked(&after), want(&after));
        // A short APP0 whose identifier runs past its end, as jpeg-rs reads it: dropped.
        let mut short = vec![0xff, 0xe0, 0, 2];
        short.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        assert_eq!(
            marked(&[short, sof.clone(), sos.clone()]),
            want(&[b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0".to_vec(), sof, sos])
        );
    }

    // The marked stream reads RGB in jpeg-rs's default model, also when it had JFIF.
    #[test]
    fn mark_rgb_reads_rgb() {
        let m = jpeg::Rgba {
            pix: vec![90; 4 * 16 * 16],
            stride: 64,
            rect: jpeg::Rect::new(0, 0, 16, 16),
        };
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let b = jpeg::encode(
            jpeg::EncodeImage::Rgba(&m),
            &jpeg::EncodeOptions {
                quality: 90,
                chroma444: true,
            },
            &jpeg::Limits::default(),
            &cancel,
        )
        .unwrap();
        let mut j = SOI.to_vec();
        j.extend(seg(0xe0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0"));
        j.extend_from_slice(&b[2..]);
        for mut s in [b, j] {
            s.reserve(APP14_RGB.len());
            mark_rgb(&mut s);
            let cfg = jpeg::decode_config(&s, None, &jpeg::Limits::default()).unwrap();
            assert!(cfg.rgb);
        }
    }
}
