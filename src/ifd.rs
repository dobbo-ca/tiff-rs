//! The TIFF container: header, IFD chain and tag values, ported from tiff-go's `internal/ifd`
//! (`ifd.go`). It never allocates in proportion to a count read from the file.

use crate::error::{Error, corrupt, limit};
use crate::readat::{ReadAt, SectionReader, read_exact_at};
use std::collections::{HashMap, HashSet};

/// Bounds one IFD's tag count (tags are unique, so a real IFD holds a few dozen).
const MAX_ENTRIES: u64 = 4096;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ByteOrder {
    Little,
    Big,
}

impl ByteOrder {
    pub(crate) fn u16(self, b: &[u8]) -> u16 {
        let a = [b[0], b[1]];
        match self {
            Self::Little => u16::from_le_bytes(a),
            Self::Big => u16::from_be_bytes(a),
        }
    }

    pub(crate) fn u32(self, b: &[u8]) -> u32 {
        let a = [b[0], b[1], b[2], b[3]];
        match self {
            Self::Little => u32::from_le_bytes(a),
            Self::Big => u32::from_be_bytes(a),
        }
    }

    pub(crate) fn u64(self, b: &[u8]) -> u64 {
        let a = [b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]];
        match self {
            Self::Little => u64::from_le_bytes(a),
            Self::Big => u64::from_be_bytes(a),
        }
    }

    pub(crate) fn put_u16(self, b: &mut [u8], v: u16) {
        let a = match self {
            Self::Little => v.to_le_bytes(),
            Self::Big => v.to_be_bytes(),
        };
        b[..2].copy_from_slice(&a);
    }

    /// The unsigned integer of `b`, which is 1, 2, 4 or 8 bytes.
    fn uint(self, b: &[u8]) -> u64 {
        match b.len() {
            1 => u64::from(b[0]),
            2 => u64::from(self.u16(b)),
            4 => u64::from(self.u32(b)),
            _ => self.u64(b),
        }
    }
}

/// An opened container: byte order, variant and the IFD chain.
pub(crate) struct Container<R> {
    ra: R,
    size: i64,
    pub(crate) bo: ByteOrder,
    big: bool,
    /// Each IFD's offset in chain order.
    pub(crate) offs: Vec<i64>,
}

/// One directory entry.
#[derive(Clone, Copy, Default)]
pub(crate) struct Entry {
    pub(crate) tag: u16,
    pub(crate) typ: u16,
    pub(crate) count: u64,
    pub(crate) sz: u64,
    inline: [u8; 8],
    is_inline: bool,
    off: i64,
    /// Count or offset outside the file.
    pub(crate) bad: bool,
}

fn type_size(t: u16) -> u64 {
    match t {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 | 16..=18 => 8,
        _ => 0,
    }
}

/// What `entries` returns for one directory.
pub(crate) struct Entries {
    pub(crate) map: HashMap<u16, Entry>,
    /// Ids of the tags whose count or offset lies outside the file, in file order.
    pub(crate) skipped: Vec<u16>,
    /// The Orientation and XMP entries, unmapped and in file order, whatever their type.
    pub(crate) tagged: Vec<Entry>,
}

impl<R: ReadAt> Container<R> {
    pub(crate) fn read_at(&self, p: &mut [u8], off: i64) -> Result<(), Error> {
        if !(0..=self.size).contains(&off) || p.len() as i64 > self.size - off {
            return Err(corrupt("read outside file"));
        }
        read_exact_at(&self.ra, p, off as u64).map_err(|e| Error::Io(e.kind()))
    }

    /// Reads the header and walks the IFD chain, refusing loops and more than `max_ifds`
    /// directories. No directory body is parsed.
    pub(crate) fn open(ra: R, size: u64, max_ifds: usize) -> Result<Self, Error> {
        let size = i64::try_from(size).map_err(|_| corrupt("file size"))?;
        let mut f = Container {
            ra,
            size,
            bo: ByteOrder::Little,
            big: false,
            offs: Vec::new(),
        };
        if size < 8 {
            return Err(corrupt("short header"));
        }
        let mut h = [0u8; 16];
        f.read_at(&mut h[..size.min(16) as usize], 0)?;
        f.bo = match &h[..2] {
            b"II" => ByteOrder::Little,
            b"MM" => ByteOrder::Big,
            _ => return Err(corrupt("byte order mark")),
        };
        let first = match f.bo.u16(&h[2..]) {
            42 => u64::from(f.bo.u32(&h[4..])),
            43 => {
                if size < 16 || f.bo.u16(&h[4..]) != 8 || f.bo.u16(&h[6..]) != 0 {
                    return Err(corrupt("bigtiff header"));
                }
                f.big = true;
                f.bo.u64(&h[8..])
            }
            _ => return Err(corrupt("version")),
        };
        let mut seen = HashSet::new();
        let mut off = first;
        while off != 0 {
            if !seen.insert(off) {
                return Err(corrupt("ifd chain loop"));
            }
            if f.offs.len() >= max_ifds {
                return Err(limit("ifds", f.offs.len() as u64 + 1, max_ifds as u64));
            }
            if off >= size as u64 {
                return Err(corrupt("ifd offset"));
            }
            f.offs.push(off as i64);
            let (n, start) = f.count(off as i64)?;
            let osz = f.off_size();
            let mut b = [0u8; 8];
            f.read_at(&mut b[..osz as usize], start + n * f.entry_size())?;
            off = f.bo.uint(&b[..osz as usize]);
        }
        if f.offs.is_empty() {
            return Err(corrupt("no ifd"));
        }
        Ok(f)
    }

    fn off_size(&self) -> i64 {
        if self.big { 8 } else { 4 }
    }

    fn entry_size(&self) -> i64 {
        if self.big { 20 } else { 12 }
    }

    /// The entry count and the offset of the first entry.
    fn count(&self, off: i64) -> Result<(i64, i64), Error> {
        let mut b = [0u8; 8];
        let w = if self.big { 8 } else { 2 };
        self.read_at(&mut b[..w as usize], off)?;
        let c = self.bo.uint(&b[..w as usize]);
        if c > MAX_ENTRIES {
            return Err(limit("ifd entries", c.min(1 << 62), MAX_ENTRIES));
        }
        if c == 0 {
            return Err(corrupt("empty ifd"));
        }
        let n = c as i64;
        let need = w + n * self.entry_size() + self.off_size();
        if off > self.size - need {
            return Err(corrupt("ifd past end of file"));
        }
        Ok((n, off + w))
    }

    /// The directory's tags. A tag whose count or offset lies outside the file is kept with
    /// `bad` set: reading its value is corrupt, so only the tags a page reads can make it
    /// corrupt.
    pub(crate) fn entries(&self, off: i64) -> Result<Entries, Error> {
        let (n, start) = self.count(off)?;
        let es = self.entry_size() as usize;
        let mut buf = vec![0u8; n as usize * es];
        self.read_at(&mut buf, start)?;
        let mut out = Entries {
            map: HashMap::with_capacity(n as usize),
            skipped: Vec::new(),
            tagged: Vec::new(),
        };
        let vw = self.off_size() as usize;
        for rec in buf.chunks_exact(es) {
            let (tag, typ) = (self.bo.u16(rec), self.bo.u16(&rec[2..]));
            let (count, val) = if self.big {
                (self.bo.u64(&rec[4..]), &rec[12..])
            } else {
                (u64::from(self.bo.u32(&rec[4..])), &rec[8..])
            };
            let mut en = Entry {
                tag,
                typ,
                count,
                sz: type_size(typ),
                inline: [0; 8],
                is_inline: false,
                off: 0,
                bad: false,
            };
            // Range checks divide instead of multiplying: count * sz can wrap.
            if en.sz == 0 {
                // unknown type: ignore the tag
            } else if en.count <= vw as u64 / en.sz {
                en.is_inline = true;
                en.inline[..vw].copy_from_slice(&val[..vw]);
            } else {
                let o = self.bo.uint(&val[..vw]);
                en.bad = o > self.size as u64 || en.count > (self.size as u64 - o) / en.sz;
                if !en.bad {
                    en.off = o as i64;
                }
            }
            if en.bad {
                out.skipped.push(en.tag);
            }
            if en.tag == 274 || en.tag == 700 {
                out.tagged.push(en);
            }
            if en.sz != 0 {
                out.map.insert(en.tag, en);
            }
        }
        Ok(out)
    }

    /// Integer element `i` of an unsigned-integer entry (BYTE, SHORT, LONG, LONG8), read from
    /// the file only when it is not inline.
    pub(crate) fn val(&self, en: &Entry, i: u64) -> Result<u64, Error> {
        if en.bad {
            return Err(corrupt(format!("tag {} value outside file", en.tag)));
        }
        if !matches!(en.typ, 1 | 3 | 4 | 16) {
            return Err(corrupt(format!("tag {} type", en.tag)));
        }
        if i >= en.count {
            return Err(corrupt(format!("tag {} index", en.tag)));
        }
        let (a, b) = ((i * en.sz) as usize, ((i + 1) * en.sz) as usize);
        if en.is_inline {
            return Ok(self.bo.uint(&en.inline[a..b]));
        }
        let mut v = [0u8; 8];
        self.read_at(&mut v[..en.sz as usize], en.off + (i * en.sz) as i64)?;
        Ok(self.bo.uint(&v[..en.sz as usize]))
    }

    /// An entry's whole value; the caller has bounded `count`.
    pub(crate) fn bytes(&self, en: &Entry) -> Result<Vec<u8>, Error> {
        if en.bad {
            return Err(corrupt(format!("tag {} value outside file", en.tag)));
        }
        let n = en
            .count
            .checked_mul(en.sz)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or(Error::OutOfMemory)?;
        if en.is_inline {
            return Ok(en.inline[..n].to_vec());
        }
        let mut b = Vec::new();
        b.try_reserve_exact(n).map_err(|_| Error::OutOfMemory)?;
        b.resize(n, 0);
        self.read_at(&mut b, en.off)?;
        Ok(b)
    }

    /// An unsigned-integer entry with at most `max` elements.
    pub(crate) fn ints(&self, en: &Entry, max: u64) -> Result<Vec<u64>, Error> {
        if en.count > max {
            return Err(corrupt(format!("tag {} count", en.tag)));
        }
        if !matches!(en.typ, 1 | 3 | 4 | 16) {
            return Err(corrupt(format!("tag {} type", en.tag)));
        }
        let b = self.bytes(en)?;
        Ok(b.chunks_exact(en.sz as usize)
            .map(|c| self.bo.uint(c))
            .collect())
    }

    /// The validated file range of strip or tile `i`.
    pub(crate) fn chunk(&self, offs: &Entry, cnts: &Entry, i: u64) -> Result<(i64, i64), Error> {
        let o = self.val(offs, i)?;
        let c = self.val(cnts, i)?;
        if o > self.size as u64 || c > self.size as u64 - o {
            return Err(corrupt(format!("chunk {i} outside file")));
        }
        Ok((o as i64, c as i64))
    }

    /// A reader over `[off, off+n)`.
    pub(crate) fn section(&self, off: i64, n: i64) -> SectionReader<'_, R> {
        SectionReader::new(&self.ra, off as u64, n as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Go's TestReadAtNearMaxOffset.
    #[test]
    fn read_at_near_max_offset() {
        let f = Container {
            ra: vec![0u8; 16],
            size: 16,
            bo: ByteOrder::Little,
            big: false,
            offs: Vec::new(),
        };
        for off in [i64::MAX - 7, i64::MAX, 1 << 62, 15] {
            assert!(matches!(
                f.read_at(&mut [0u8; 16], off),
                Err(Error::Corrupt(_))
            ));
        }
        f.read_at(&mut [0u8; 16], 0).unwrap();
    }
}
