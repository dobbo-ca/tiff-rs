//! The Apple PackBits run-length scheme, streamed.

use super::{check, read_byte};
use std::io::{self, BufRead, Read};
use std::sync::atomic::AtomicBool;

pub(crate) struct PackBits<'a, R> {
    r: R,
    cancel: &'a AtomicBool,
    /// Literal bytes left in the current packet.
    lit: usize,
    /// Repeats left in the current run.
    rep: usize,
    /// The repeated byte.
    b: u8,
}

impl<'a, R: BufRead> PackBits<'a, R> {
    pub(super) fn new(r: R, cancel: &'a AtomicBool) -> Self {
        Self {
            r,
            cancel,
            lit: 0,
            rep: 0,
            b: 0,
        }
    }
}

impl<R: BufRead> Read for PackBits<'_, R> {
    fn read(&mut self, p: &mut [u8]) -> io::Result<usize> {
        let mut n = 0;
        while n < p.len() {
            if self.lit > 0 {
                let m = self.lit.min(p.len() - n);
                self.r.read_exact(&mut p[n..n + m])?;
                n += m;
                self.lit -= m;
            } else if self.rep > 0 {
                let m = self.rep.min(p.len() - n);
                p[n..n + m].fill(self.b);
                n += m;
                self.rep -= m;
            } else {
                // A 0x80 header makes no output: a long run of them must be cancellable.
                check(self.cancel).map_err(io::Error::other)?;
                // At the end of the data a short read ends the stream; a run header whose
                // byte is missing leaves the run set, as the Go decoder does.
                let Some(c) = read_byte(&mut self.r)? else {
                    return Ok(n);
                };
                if c < 128 {
                    self.lit = usize::from(c) + 1;
                } else if c > 128 {
                    self.rep = 257 - usize::from(c);
                    let Some(b) = read_byte(&mut self.r)? else {
                        return Ok(n);
                    };
                    self.b = b;
                }
            }
        }
        Ok(n)
    }
}
