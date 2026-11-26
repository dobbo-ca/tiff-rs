use std::io::{self, Read};

/// Positioned reads, Go's `io.ReaderAt` without the EOF rule: a short read is a read that
/// returns fewer bytes than asked, and 0 means the end of the source. Like Go's `io.ReaderAt`,
/// calls may come from several threads at once (`Sync`): a decode with `max_threads` above 1
/// reads strips and tiles in parallel.
///
/// As in Go, the result of a call must depend only on `offset` and `buf.len()` (the same source
/// gives the same bytes or the same error for the same read, whatever else was read before or
/// at the same time). The guarantee that every `max_threads` gives the same bands, errors and
/// partial output assumes it: a source whose failures depend on the number or the order of the
/// calls (a dropped connection, a fault injected after N reads) can fail at another chunk when
/// the chunks are read in parallel, and with fewer bands delivered.
pub trait ReadAt: Sync {
    /// Reads into `buf` from `offset`; returns how many bytes it read.
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize>;
}

impl ReadAt for [u8] {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        let Some(src) = usize::try_from(offset).ok().and_then(|o| self.get(o..)) else {
            return Ok(0);
        };
        let n = src.len().min(buf.len());
        buf[..n].copy_from_slice(&src[..n]);
        Ok(n)
    }
}

impl ReadAt for Vec<u8> {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        self.as_slice().read_at(buf, offset)
    }
}

impl<T: ReadAt + ?Sized> ReadAt for &T {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        (**self).read_at(buf, offset)
    }
}

#[cfg(unix)]
impl ReadAt for std::fs::File {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        std::os::unix::fs::FileExt::read_at(self, buf, offset)
    }
}

#[cfg(windows)]
impl ReadAt for std::fs::File {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        std::os::windows::fs::FileExt::seek_read(self, buf, offset)
    }
}

/// Fills `buf` from `offset`, or fails with `UnexpectedEof`.
pub(crate) fn read_exact_at<R: ReadAt + ?Sized>(
    ra: &R,
    mut buf: &mut [u8],
    mut offset: u64,
) -> io::Result<()> {
    while !buf.is_empty() {
        match ra.read_at(buf, offset) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                buf = &mut buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Go's `io.SectionReader`: the bytes `[pos, end)` of a [`ReadAt`].
pub(crate) struct SectionReader<'a, R> {
    ra: &'a R,
    pos: u64,
    end: u64,
}

impl<'a, R: ReadAt> SectionReader<'a, R> {
    pub(crate) fn new(ra: &'a R, off: u64, n: u64) -> Self {
        Self {
            ra,
            pos: off,
            end: off.saturating_add(n),
        }
    }

    /// Bytes left.
    pub(crate) fn len(&self) -> u64 {
        self.end - self.pos
    }
}

impl<R> Clone for SectionReader<'_, R> {
    fn clone(&self) -> Self {
        Self {
            ra: self.ra,
            pos: self.pos,
            end: self.end,
        }
    }
}

impl<R: ReadAt> Read for SectionReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = (buf.len() as u64).min(self.end - self.pos) as usize;
        if n == 0 {
            return Ok(0);
        }
        let n = self.ra.read_at(&mut buf[..n], self.pos)?;
        self.pos += n as u64;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_reads_stop_at_the_end() {
        let v = vec![1u8, 2, 3];
        let mut b = [0u8; 4];
        assert_eq!(v.read_at(&mut b, 1).unwrap(), 2);
        assert_eq!(&b[..2], &[2, 3]);
        assert_eq!(v.read_at(&mut b, 3).unwrap(), 0);
        assert_eq!(v.read_at(&mut b, u64::MAX).unwrap(), 0);
        assert!(read_exact_at(&v, &mut b, 0).is_err());
    }

    #[test]
    fn section_is_bounded() {
        let v = vec![1u8, 2, 3, 4, 5];
        let mut s = SectionReader::new(&v, 1, 3);
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        assert_eq!(out, [2, 3, 4]);
    }
}
