//! The working memory of one call: every buffer that scales with the input is charged before it
//! is allocated and released when it drops, by all threads of the call together.

use crate::error::{Error, limit};
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU64, Ordering};

/// Bytes booked against `Limits::max_alloc`, shared by every thread of one call.
pub(crate) struct Ledger {
    max: u64,
    used: AtomicU64,
    peak: AtomicU64,
}

impl Ledger {
    pub(crate) fn new(max: u64) -> Self {
        Self {
            max,
            used: AtomicU64::new(0),
            peak: AtomicU64::new(0),
        }
    }

    /// What may still be charged.
    pub(crate) fn left(&self) -> u64 {
        self.max.saturating_sub(self.used.load(Ordering::Relaxed))
    }

    /// The most bytes booked at once so far.
    pub(crate) fn peak(&self) -> u64 {
        self.peak.load(Ordering::Relaxed)
    }

    /// Books `n` bytes until the returned guard drops. Over the limit it is the `alloc` limit
    /// error that names `n` and what was left.
    pub(crate) fn charge(&self, n: u64) -> Result<Held<'_>, Error> {
        let r = self
            .used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |u| {
                u.checked_add(n).filter(|&t| t <= self.max)
            });
        match r {
            Ok(u) => {
                self.peak.fetch_max(u + n, Ordering::Relaxed);
                Ok(Held { l: self, n })
            }
            Err(u) => Err(limit("alloc", n, self.max.saturating_sub(u))),
        }
    }
}

/// Bytes booked in a [`Ledger`], released on drop.
pub(crate) struct Held<'a> {
    l: &'a Ledger,
    n: u64,
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        self.l.used.fetch_sub(self.n, Ordering::AcqRel);
    }
}

impl Held<'_> {
    /// Gives back `n` of the booked bytes, or all of them if it holds fewer.
    pub(crate) fn release(&mut self, n: u64) {
        let n = n.min(self.n);
        self.l.used.fetch_sub(n, Ordering::AcqRel);
        self.n -= n;
    }

    /// Raises the booking to `n` bytes in all, or leaves it as it is and fails if the ledger has
    /// no room for the difference.
    pub(crate) fn raise(&mut self, n: u64) -> Result<(), Error> {
        if n > self.n {
            let mut more = self.l.charge(n - self.n)?;
            self.n += std::mem::take(&mut more.n);
        }
        Ok(())
    }
}

/// A zeroed buffer whose size is booked in the ledger for as long as it lives.
pub(crate) struct Buf<'a> {
    v: Vec<u8>,
    held: Held<'a>,
}

impl<'a> Buf<'a> {
    /// `n` zero bytes, `held` being the charge for them.
    pub(crate) fn new(n: usize, held: Held<'a>) -> Result<Self, Error> {
        let mut v = Vec::new();
        v.try_reserve_exact(n).map_err(|_| Error::OutOfMemory)?;
        v.resize(n, 0);
        Ok(Self { v, held })
    }

    /// The bytes and the charge, to keep them apart.
    pub(crate) fn into_parts(self) -> (Vec<u8>, Held<'a>) {
        (self.v, self.held)
    }
}

impl Deref for Buf<'_> {
    type Target = Vec<u8>;
    fn deref(&self) -> &Vec<u8> {
        &self.v
    }
}

impl DerefMut for Buf<'_> {
    fn deref_mut(&mut self) -> &mut Vec<u8> {
        &mut self.v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charges_release_and_peak() {
        let l = Ledger::new(100);
        let a = l.charge(60).unwrap();
        assert_eq!(
            l.charge(50).err(),
            Some(Error::Limit {
                name: "alloc",
                got: 50,
                max: 40
            })
        );
        drop(a);
        assert_eq!(l.left(), 100);
        let _b = l.charge(100).unwrap();
        assert_eq!(l.peak(), 100);
    }

    #[test]
    fn held_release_and_raise() {
        let l = Ledger::new(100);
        let mut a = l.charge(40).unwrap();
        a.raise(70).unwrap();
        assert_eq!(l.left(), 30);
        assert!(a.raise(120).is_err());
        assert_eq!(l.left(), 30);
        a.release(50);
        assert_eq!(l.left(), 80);
        a.release(500);
        assert_eq!(l.left(), 100);
        drop(a);
        assert_eq!(l.left(), 100);
    }
}
