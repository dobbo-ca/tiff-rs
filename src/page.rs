//! One parsed IFD, ported from tiff-go's `internal/ifd/page.go`.

use crate::Limits;
use crate::error::{Error, corrupt, limit, unsupported};
use crate::ifd::{Container, Entry};
use crate::readat::ReadAt;
use std::collections::HashMap;

// Compression codes this crate decodes.
pub(crate) const NONE: u64 = 1;
pub(crate) const CCITT3: u64 = 3;
pub(crate) const CCITT4: u64 = 4;
pub(crate) const LZW: u64 = 5;
pub(crate) const JPEG: u64 = 7;
pub(crate) const DEFLATE: u64 = 8;
/// Adobe's code for the same zlib stream.
pub(crate) const DEFLATE_X: u64 = 32946;
pub(crate) const PACKBITS: u64 = 32773;

// Photometric values this crate decodes (YCbCr only inside JPEG).
pub(crate) const WHITE_IS_ZERO: u64 = 0;
pub(crate) const BLACK_IS_ZERO: u64 = 1;
pub(crate) const RGB: u64 = 2;
pub(crate) const PALETTE: u64 = 3;
pub(crate) const CMYK: u64 = 5;
pub(crate) const YCBCR: u64 = 6;

/// One Orientation (274) or XMP (700) entry as stored. See [`Page::orientations`](crate::Page).
///
/// `value` is an Orientation's first element (saturated at `i32::MAX`), `data` an XMP value;
/// each is empty when the entry is `out_of_range`, a `bad_type`, or (`data`) `over_limit` of
/// `Limits::max_metadata_bytes`. `duplicate` is true on every entry when the page has more than
/// one of the tag.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TagEntry {
    pub typ: u16,
    pub count: u64,
    /// An Orientation's first element, saturated at `i32::MAX`; 0 when it was not read.
    pub value: u32,
    /// An XMP value; `None` when it was not read.
    pub data: Option<Vec<u8>>,
    /// Count or offset lies outside the file.
    pub out_of_range: bool,
    /// Not a type the tag may have.
    pub bad_type: bool,
    /// `data` exceeds the metadata budget.
    pub over_limit: bool,
    /// The page holds more than one entry of this tag.
    pub duplicate: bool,
}

/// One parsed IFD.
pub(crate) struct Page {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) bps: usize,
    pub(crate) spp: usize,
    pub(crate) photometric: u64,
    pub(crate) compression: u64,
    pub(crate) predictor: u64,
    pub(crate) orientation: u64,
    pub(crate) t4: u64,
    pub(crate) color_map: Vec<u16>,
    pub(crate) extra_samples: Vec<u16>,
    pub(crate) jpeg_tables: Vec<u8>,
    /// Tags dropped for a count or offset outside the file; a later valid duplicate is read
    /// instead and not listed.
    pub(crate) skipped: Vec<u16>,
    pub(crate) orientations: Vec<TagEntry>,
    pub(crate) xmps: Vec<TagEntry>,
    /// XResolution, YResolution: 0 when absent or not one valid RATIONAL.
    pub(crate) x_res: f64,
    pub(crate) y_res: f64,
    /// ResolutionUnit, 2 (inch) when absent or not a SHORT.
    pub(crate) res_unit: u64,
    /// Zero for strips.
    pub(crate) tile_w: usize,
    pub(crate) tile_l: usize,
    pub(crate) rows_per_strip: usize,
    pub(crate) offsets: Entry,
    pub(crate) counts: Entry,
    pub(crate) across: u64,
    pub(crate) down: u64,
    n_strips: u64,
}

impl Page {
    /// How many strips or tiles the page has.
    pub(crate) fn chunks(&self) -> u64 {
        if self.tile_w > 0 {
            self.across * self.down
        } else {
            self.n_strips
        }
    }

    /// The packed size of one full-width row.
    pub(crate) fn row_bytes(&self) -> u64 {
        (self.width as u64 * self.spp as u64 * self.bps as u64).div_ceil(8)
    }
}

/// Charges `n` against `max_alloc` before it is allocated.
fn charge(used: &mut u64, n: u64, max: u64) -> Result<(), Error> {
    let u = used.saturating_add(n);
    if u > max {
        return Err(limit("alloc", u, max));
    }
    *used = u;
    Ok(())
}

impl<R: ReadAt> Container<R> {
    fn one(&self, m: &HashMap<u16, Entry>, tag: u16, def: u64) -> Result<u64, Error> {
        let Some(en) = m.get(&tag) else {
            return Ok(def);
        };
        // A bad Orientation or FillOrder reads as 1.
        if (tag == 274 || tag == 266) && en.bad {
            return Ok(def);
        }
        // So does a wrong type or count.
        if tag == 274 && (en.count != 1 || !matches!(en.typ, 1 | 3 | 4 | 16)) {
            return Ok(def);
        }
        if en.count != 1 {
            return Err(corrupt(format!("tag {tag} count")));
        }
        self.val(en, 0)
    }

    /// Lists the tagged entries. XMP bytes read in total stay within `budget`.
    fn meta(
        &self,
        tagged: &[Entry],
        mut budget: u64,
        used: &mut u64,
        max_alloc: u64,
    ) -> Result<(Vec<TagEntry>, Vec<TagEntry>), Error> {
        let mut n = [0usize; 2]; // entries per tag: 274, 700
        for en in tagged {
            n[usize::from(en.tag == 700)] += 1;
        }
        let (mut or, mut xmp) = (Vec::new(), Vec::new());
        for en in tagged {
            let x = en.tag == 700;
            let mut m = TagEntry {
                typ: en.typ,
                count: en.count,
                out_of_range: en.bad,
                duplicate: n[usize::from(x)] > 1,
                ..Default::default()
            };
            let ok = if x {
                matches!(en.typ, 1 | 2 | 7)
            } else {
                matches!(en.typ, 1 | 3 | 4 | 16)
            };
            m.bad_type = !ok;
            if !m.out_of_range && !m.bad_type {
                if !x {
                    if en.count > 0 {
                        m.value = self.val(en, 0)?.min(i32::MAX as u64) as u32;
                    }
                } else if en.count > budget {
                    m.over_limit = true;
                } else {
                    budget -= en.count;
                    charge(used, en.count, max_alloc)?;
                    m.data = Some(self.bytes(en)?);
                }
            }
            if x {
                xmp.push(m);
            } else {
                or.push(m);
            }
        }
        Ok((or, xmp))
    }

    /// Parses IFD `i`. `lim.max_pixels` caps Width*Height; `lim.max_metadata_bytes` caps the XMP
    /// bytes read.
    pub(crate) fn page(&self, i: usize, lim: &Limits) -> Result<Page, Error> {
        let es = self.entries(self.offs[i])?;
        let (m, skipped, tagged) = (es.map, es.skipped, es.tagged);
        let mut used = 0u64;
        let mut v = [0u64; 11];
        for (k, (tag, def)) in [
            (256u16, 0u64),
            (257, 0),
            (259, 1),
            (262, 1 << 20),
            (274, 1),
            (277, 1),
            (284, 1),
            (317, 1),
            (266, 1),
            (322, 0),
            (323, 0),
        ]
        .into_iter()
        .enumerate()
        {
            v[k] = self.one(&m, tag, def)?;
        }
        let [w, h, comp, photo, orient, spp, planar, pred, fill, tw, tl] = v;
        if w == 0 || h == 0 || w > 1 << 31 || h > 1 << 31 {
            return Err(corrupt("dimensions"));
        }
        let px = w * h;
        if px > lim.max_pixels {
            return Err(limit("pixels", px, lim.max_pixels));
        }
        if photo == 1 << 20 {
            return Err(corrupt("missing photometric"));
        }
        if spp == 0 || spp > 16 {
            return Err(unsupported(format!("samples per pixel {spp}")));
        }
        if !matches!(
            comp,
            NONE | CCITT3 | CCITT4 | LZW | JPEG | DEFLATE | DEFLATE_X | PACKBITS
        ) {
            return Err(unsupported(format!("compression {comp}")));
        }
        let min_spp = match photo {
            WHITE_IS_ZERO | BLACK_IS_ZERO | PALETTE => 1,
            RGB | YCBCR => 3,
            CMYK => 4,
            _ => 0,
        };
        if min_spp == 0 || (photo == YCBCR && comp != JPEG) {
            return Err(unsupported(format!("photometric {photo}")));
        }
        if spp < min_spp {
            return Err(corrupt(format!(
                "samples per pixel for photometric {photo}"
            )));
        }
        if planar != 1 {
            return Err(unsupported(format!("planar configuration {planar}")));
        }
        if fill != 1 && fill != 2 {
            return Err(unsupported(format!("fill order {fill}")));
        }
        if pred != 1 && pred != 2 {
            return Err(unsupported(format!("predictor {pred}")));
        }
        if let Some(en) = m.get(&339) {
            for s in self.ints(en, 16)? {
                if s != 1 {
                    return Err(unsupported(format!("sample format {s}")));
                }
            }
        }

        let mut bps = 1u64;
        if let Some(en) = m.get(&258) {
            let bs = self.ints(en, 16)?;
            if bs.len() != 1 && bs.len() as u64 != spp {
                return Err(corrupt("bits per sample count"));
            }
            if bs.iter().any(|&b| b != bs[0]) {
                return Err(unsupported("mixed bits per sample"));
            }
            if !(1..=16).contains(&bs[0]) {
                return Err(unsupported(format!("bits per sample {}", bs[0])));
            }
            bps = bs[0];
        }
        // libtiff ignores it at 8 bits and up.
        if fill == 2 && bps < 8 {
            return Err(unsupported(format!("fill order {fill} at {bps} bits")));
        }
        if pred == 2 && bps != 8 && bps != 16 {
            return Err(unsupported(format!("predictor at {bps} bits")));
        }
        if comp == JPEG && bps != 8 {
            return Err(unsupported(format!("jpeg at {bps} bits")));
        }
        let mut p = Page {
            width: w as usize,
            height: h as usize,
            bps: bps as usize,
            spp: spp as usize,
            photometric: photo,
            compression: comp,
            predictor: pred,
            orientation: orient,
            t4: 0,
            color_map: Vec::new(),
            extra_samples: Vec::new(),
            jpeg_tables: Vec::new(),
            skipped: Vec::new(),
            orientations: Vec::new(),
            xmps: Vec::new(),
            x_res: 0.0,
            y_res: 0.0,
            res_unit: 2,
            tile_w: 0,
            tile_l: 0,
            rows_per_strip: 0,
            offsets: Entry::default(),
            counts: Entry::default(),
            across: 0,
            down: 0,
            n_strips: 0,
        };
        if photo == PALETTE {
            let en = match m.get(&320) {
                Some(en) if bps <= 8 && en.count == 3 << bps => en,
                _ => return Err(corrupt("colour map")),
            };
            p.color_map = self
                .ints(en, 3 << 8)?
                .into_iter()
                .map(|c| c as u16)
                .collect();
        }
        if let Some(en) = m.get(&338) {
            p.extra_samples = self.ints(en, 16)?.into_iter().map(|x| x as u16).collect();
        }
        if let Some(en) = m.get(&347).filter(|_| comp == JPEG) {
            // A bad entry is corrupt, via bytes.
            if en.count > 1 << 20 && !en.bad {
                return Err(limit("jpeg tables", en.count.min(1 << 62), 1 << 20));
            }
            if !en.bad {
                charge(&mut used, en.count.saturating_mul(en.sz), lim.max_alloc)?;
            }
            p.jpeg_tables = self.bytes(en)?;
        }
        if comp == CCITT3 || comp == CCITT4 {
            // As Go: only an entry outside the file is an error; any other unreadable value
            // reads as 0.
            if let Some(en) = m.get(&292) {
                match self.val(en, 0) {
                    Ok(v) => p.t4 = v,
                    Err(e) if en.bad => return Err(e),
                    Err(_) => {}
                }
            }
            if let Some(en) = m.get(&293).filter(|e| e.bad) {
                self.val(en, 0)?;
            }
        }

        let (off_tag, cnt_tag, need) = if tw != 0 || tl != 0 {
            if tw == 0
                || tl == 0
                || tw > 1 << 31
                || tl > 1 << 31
                || !(tw * spp * bps).is_multiple_of(8)
            {
                return Err(corrupt("tile size"));
            }
            p.tile_w = tw as usize;
            p.tile_l = tl as usize;
            p.across = w.div_ceil(tw);
            p.down = h.div_ceil(tl);
            (324u16, 325u16, p.across * p.down)
        } else {
            let rps = self.one(&m, 278, (1 << 32) - 1)?;
            if rps == 0 {
                return Err(corrupt("rows per strip"));
            }
            p.rows_per_strip = rps.min(h) as usize;
            p.n_strips = h.div_ceil(p.rows_per_strip as u64);
            (273, 279, p.n_strips)
        };
        match (m.get(&off_tag), m.get(&cnt_tag)) {
            (Some(o), Some(c)) if !o.bad && !c.bad && o.count >= need && c.count >= need => {
                p.offsets = *o;
                p.counts = *c;
            }
            _ => return Err(corrupt("chunk table")),
        }
        p.x_res = self.rat(m.get(&282));
        p.y_res = self.rat(m.get(&283));
        if let Some(en) = m.get(&296)
            && en.typ == 3
            && en.count > 0
            && !en.bad
            && let Ok(u) = self.val(en, 0)
        {
            p.res_unit = u;
        }
        (p.orientations, p.xmps) =
            self.meta(&tagged, lim.max_metadata_bytes, &mut used, lim.max_alloc)?;
        for t in skipped {
            if m.get(&t).is_some_and(|e| e.bad) && !p.skipped.contains(&t) {
                p.skipped.push(t);
            }
        }
        Ok(p)
    }

    /// A single RATIONAL entry's value, or 0 when the entry is absent, of another type or
    /// count, outside the file, unreadable or has a zero denominator.
    fn rat(&self, en: Option<&Entry>) -> f64 {
        let Some(en) = en else { return 0.0 };
        if en.typ != 5 || en.count != 1 || en.bad {
            return 0.0;
        }
        let Ok(b) = self.bytes(en) else { return 0.0 };
        let d = self.bo.u32(&b[4..]);
        if d == 0 {
            return 0.0;
        }
        f64::from(self.bo.u32(&b)) / f64::from(d)
    }
}
