//! The worker pool and the thread gauge of a call. A call's workers are at most
//! `Limits::max_threads`, the calling thread included, and run on the caller's [`ThreadPool`] or
//! the crate's lazy shared one; the threads outlive the call, so a small page pays a wake-up, not
//! a thread start. Rayon's scope lets a pool thread borrow the call's buffers, which
//! `std::thread::scope` cannot do on a reused thread, so nothing here is `unsafe`.

use crate::Error;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

/// The stack of a pool thread: the decoders keep their large buffers on the heap.
const THREAD_STACK: usize = 512 << 10;

/// Worker threads that decodes share. Create one, keep it, and give it to
/// [`File::with_pool`](crate::File::with_pool): each call then runs its parallel passes on these
/// threads. A call without one uses the crate's lazy shared pool, which starts on the first call
/// with `max_threads` above 1 and more than one strip or tile, and never has more than
/// [`std::thread::available_parallelism`] threads; a caller's pool always wins.
/// [`Limits::max_threads`](crate::Limits::max_threads) still caps the workers of one call, the
/// calling thread included, and a call uses at most one more than the pool has threads. Clones
/// share the threads, which stop when the last clone drops. A pool is not charged to
/// `Limits::max_alloc`; its threads' stacks are its owner's.
///
/// A pool owns two sets of threads of the same count: its own, which decode strips and tiles in
/// parallel, and a [`jpeg::ThreadPool`], which a call that runs serially (one strip or tile, or
/// a strip split in several bands) with `max_threads` above 1 hands to jpeg-rs, so that the
/// parallel passes of a JPEG decode run on the caller's pool too. A call without a pool runs
/// jpeg-rs on jpeg-rs's own lazy shared pool. When tiff itself runs several workers, the nested
/// jpeg-rs calls use one thread and touch neither pool.
#[derive(Clone, Debug)]
pub struct ThreadPool {
    pool: Arc<rayon::ThreadPool>,
    /// `None` for the crate's shared pool, which serves tiff's own passes only.
    jpeg: Option<jpeg::ThreadPool>,
}

impl ThreadPool {
    /// A pool of `threads` workers (at least 1), started now.
    ///
    /// # Errors
    /// [`Error::OutOfMemory`] if the operating system will not start the threads (it starts
    /// `threads` of tiff's and `threads` of jpeg-rs's).
    pub fn new(threads: usize) -> Result<ThreadPool, Error> {
        Self::build(threads, true)
    }

    fn build(threads: usize, with_jpeg: bool) -> Result<ThreadPool, Error> {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads.max(1))
            .stack_size(THREAD_STACK)
            .thread_name(|i| format!("tiff-worker-{i}"))
            .build()
            .map_err(|_| Error::OutOfMemory)?;
        let jpeg = if with_jpeg {
            Some(jpeg::ThreadPool::new(threads.max(1)).map_err(|_| Error::OutOfMemory)?)
        } else {
            None
        };
        Ok(ThreadPool {
            pool: Arc::new(pool),
            jpeg,
        })
    }

    /// How many worker threads the pool has (of each set).
    pub fn threads(&self) -> usize {
        self.pool.current_num_threads()
    }

    /// The pool's jpeg-rs threads.
    pub(crate) fn jpeg(&self) -> Option<&jpeg::ThreadPool> {
        self.jpeg.as_ref()
    }

    pub(crate) fn scope<'scope, OP, R>(&self, op: OP) -> R
    where
        OP: FnOnce(&rayon::Scope<'scope>) -> R,
    {
        self.pool.in_place_scope(op)
    }
}

/// The crate's shared pool: built on first use, `available_parallelism` threads, no other state
/// (and no jpeg-rs threads: nested jpeg-rs calls use jpeg-rs's own shared pool).
/// `None` if the threads could not be started (calls then run serially).
static SHARED: OnceLock<Option<ThreadPool>> = OnceLock::new();

pub(crate) fn shared_pool() -> Option<&'static ThreadPool> {
    SHARED
        .get_or_init(|| ThreadPool::build(crate::default_threads(), false).ok())
        .as_ref()
}

/// Whether the shared pool has been started (for tests: `max_threads` 1, and a caller's pool,
/// never start it).
#[doc(hidden)]
pub fn shared_pool_started() -> bool {
    SHARED.get().is_some()
}

/// The threads of one call that are running now and the most at once, the calling thread
/// included. Per call, so a test can read it without global state.
pub(crate) struct Gauge {
    live: AtomicUsize,
    peak: AtomicUsize,
}

impl Gauge {
    pub(crate) fn new() -> Self {
        Self {
            live: AtomicUsize::new(1),
            peak: AtomicUsize::new(1),
        }
    }

    pub(crate) fn peak(&self) -> usize {
        self.peak.load(Ordering::Relaxed)
    }

    /// Raises the peak to `n`: the threads a nested jpeg-rs call had at once (it runs on the
    /// calling thread, which the gauge already counts).
    pub(crate) fn note(&self, n: usize) {
        self.peak.fetch_max(n, Ordering::Relaxed);
    }
}

/// A pool worker counted in a [`Gauge`] while it runs jobs.
pub(crate) struct Live<'a>(&'a Gauge);

impl<'a> Live<'a> {
    pub(crate) fn enter(g: &'a Gauge) -> Self {
        let n = g.live.fetch_add(1, Ordering::Relaxed) + 1;
        g.peak.fetch_max(n, Ordering::Relaxed);
        Live(g)
    }
}

impl Drop for Live<'_> {
    fn drop(&mut self) {
        self.0.live.fetch_sub(1, Ordering::Relaxed);
    }
}
