//! Assembles a page's strips or tiles into bands of rows, ported from tiff-go's
//! `internal/decode`.
//!
//! With `max_threads` above 1 and several strips or tiles, the chunks are decoded on a pool, one
//! chunk per job, and the calling thread delivers them in order, so the bands are those of the
//! serial loop. A job that fails for any reason (a limit, bad data, a cancel, the reader) is not
//! reported: the caller stops the workers, releases everything they hold and runs the serial loop
//! from that chunk, so every error comes from the serial code and is the one `max_threads = 1`
//! gives.

use crate::codec::{self, Env, Params, RowReader, check, nested_need};
use crate::error::{Error, limit, unsupported};
use crate::ifd::{ByteOrder, Container};
use crate::ledger::{Buf, Ledger};
use crate::page::{CCITT3, CCITT4, JPEG, Page, RGB};
use crate::par::{Gauge, Live, ThreadPool, shared_pool};
use crate::readat::ReadAt;
use crate::{Limits, Probe};
use std::collections::BTreeMap;
use std::ops::ControlFlow;
use std::sync::atomic::AtomicBool;
use std::sync::{Condvar, Mutex, MutexGuard};

/// What one `rows` call works with.
struct Cx<'a, R> {
    c: &'a Container<R>,
    p: &'a Page,
    lim: &'a Limits,
    pool: Option<&'a ThreadPool>,
    cancel: &'a AtomicBool,
    ledger: &'a Ledger,
    gauge: &'a Gauge,
    /// Probe mode: nested jpeg-rs calls report their threads in `gauge`.
    probe: bool,
}

/// The environment of the serial loop: nested jpeg-rs calls get the call's `max_threads` and, if
/// the caller gave one, its pool.
fn serial_env<'a, R: ReadAt>(cx: &Cx<'a, R>) -> Env<'a> {
    Env::new(cx.ledger, cx.lim.max_threads).with_jpeg(cx.pool, cx.probe.then_some(cx.gauge))
}

/// How a pass over the chunks ended.
enum Next {
    Done,
    /// Run the serial loop from this strip, or this tile row.
    Retry(usize),
}

fn params(
    p: &Page,
    w: usize,
    h: usize,
    band_bytes: u64,
    jpeg_bytes: u64,
    alloc_left: u64,
) -> Params<'_> {
    Params {
        compression: p.compression,
        t4: p.t4,
        spp: p.spp,
        bps: p.bps,
        w,
        h,
        jpeg_tables: &p.jpeg_tables,
        jpeg_rgb: p.photometric == RGB,
        band_bytes,
        jpeg_bytes,
        alloc_left,
    }
}

/// The decoded size a JPEG call on a worker is budgeted for: the chunk's own, at most a band.
fn jpeg_bytes(band: u64, w: usize, h: usize) -> u64 {
    band.min(4 * w as u64 * h as u64)
}

/// Undoes the predictor and puts 16-bit samples in big-endian order. JPEG and CCITT output
/// needs neither.
fn fix(bo: ByteOrder, p: &Page, row: &mut [u8], width_px: usize) {
    if p.compression == JPEG || p.compression == CCITT3 || p.compression == CCITT4 {
        return;
    }
    let n = width_px * p.spp;
    if p.predictor == 2 && p.bps == 8 {
        for i in p.spp..n {
            row[i] = row[i].wrapping_add(row[i - p.spp]);
        }
    } else if p.predictor == 2 && p.bps == 16 {
        for i in p.spp..n {
            let v = bo
                .u16(&row[2 * i..])
                .wrapping_add(bo.u16(&row[2 * (i - p.spp)..]));
            bo.put_u16(&mut row[2 * i..], v);
        }
    }
    if p.bps == 16 && bo == ByteOrder::Little {
        for i in 0..n {
            row.swap(2 * i, 2 * i + 1);
        }
    }
}

/// Reads the first `rows` rows of the chunk `rr` opens into `dst`, `rbc` bytes each, `w` pixels
/// wide, and undoes the predictor and the byte order of each row (JPEG and CCITT rows need
/// neither). Both the workers of `rows` and `chunk_into` decode a chunk through it.
fn fill_rows<RR: RowReader>(
    rr: &mut RR,
    bo: ByteOrder,
    p: &Page,
    w: usize,
    rows: usize,
    rbc: usize,
    dst: &mut [u8],
) -> Result<(), Error> {
    if p.compression == JPEG {
        return rr.read_rows(&mut dst[..rows * rbc], rbc);
    }
    for r in 0..rows {
        let row = &mut dst[r * rbc..(r + 1) * rbc];
        rr.read_row(row)?;
        fix(bo, p, row, w);
    }
    Ok(())
}

/// Decodes chunk `i` into `out`, every row of it as stored, on the calling thread, with the codec
/// state `st` (which comes back for the next call, whatever the result). The call has a ledger of
/// its own under `lim.max_alloc`, which first books what `st` kept from earlier calls (and frees
/// what does not fit); the chunk's size is checked against `lim.max_pixels` and
/// `lim.max_band_bytes` first. A tile's rows past the image edge are as stored when the stream
/// holds them; if a data or reader error comes after the last visible row, the rest of them are
/// zero and the result is Ok, as `rows`, which never reads them, gives no error. `out` is empty
/// after an error.
pub(crate) fn chunk_into<R: ReadAt>(
    c: &Container<R>,
    p: &Page,
    lim: &Limits,
    i: usize,
    st: codec::State,
    out: &mut Vec<u8>,
    cancel: &AtomicBool,
) -> (Result<(), Error>, codec::State) {
    let ledger = Ledger::new(lim.max_alloc);
    // Nested jpeg-rs calls run on this thread alone, and touch no pool.
    let mut env = Env::with_state(&ledger, 1, st);
    env.book_retained();
    let r = chunk_run(c, p, lim, i, &mut env, out, cancel);
    if r.is_err() {
        out.clear();
    }
    (r, env.into_state())
}

fn chunk_run<R: ReadAt>(
    c: &Container<R>,
    p: &Page,
    lim: &Limits,
    i: usize,
    env: &mut Env<'_>,
    out: &mut Vec<u8>,
    cancel: &AtomicBool,
) -> Result<(), Error> {
    if i as u64 >= p.chunks() {
        return Err(unsupported(format!(
            "chunk {i} of a page with {} chunks",
            p.chunks()
        )));
    }
    check(cancel)?;
    let (w, h) = if p.tile_w > 0 {
        (p.tile_w, p.tile_l)
    } else {
        (
            p.width,
            p.rows_per_strip.min(p.height - i * p.rows_per_strip),
        )
    };
    let band = lim.max_band_bytes;
    let rbc = (w as u64 * p.spp as u64 * p.bps as u64).div_ceil(8);
    let n = rbc.saturating_mul(h as u64);
    let px = (w as u64).saturating_mul(h as u64);
    if px > lim.max_pixels {
        return Err(limit("pixels", px, lim.max_pixels));
    }
    if n > band {
        return Err(limit("band bytes", n, band));
    }
    // The output is booked in the ledger like a band buffer; the codec gets what is left.
    let _held = env.charge(n)?;
    let nu = usize::try_from(n).map_err(|_| Error::OutOfMemory)?;
    out.clear();
    out.try_reserve_exact(nu).map_err(|_| Error::OutOfMemory)?;
    out.resize(nu, 0);
    let (off, len) = c.chunk(&p.offsets, &p.counts, i as u64)?;
    let prm = params(
        p,
        w,
        h,
        band,
        jpeg_bytes(band, w, h),
        lim.max_alloc.saturating_sub(n),
    );
    let mut rr = codec::new(c.section(off, len), &prm, cancel, env)?;
    let rbu = rbc as usize;
    // The rows inside the image: those of `rows`, and the only ones whose errors count.
    let vis = if p.tile_w > 0 {
        h.min(p.height - (i / p.across as usize) * p.tile_l)
    } else {
        h
    };
    let (head, tail) = out.split_at_mut(vis * rbu);
    fill_rows(&mut rr, c.bo, p, w, vis, rbu, head)?;
    // Past the image edge, row by row: a damaged stream ends the rows it holds, the rest stay zero.
    for row in tail.chunks_exact_mut(rbu) {
        match fill_rows(&mut rr, c.bo, p, w, 1, rbu, row) {
            Ok(()) => {}
            Err(Error::Corrupt(_) | Error::Io(_)) => {
                row.fill(0);
                break;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Calls `f` with consecutive bands of full-width rows, top to bottom. No band exceeds
/// `lim.max_band_bytes`. Also reports what the call used; `probe` has the nested jpeg-rs calls
/// counted in `threads_peak` (they decode through jpeg-rs's probe entry point).
pub(crate) fn rows<R: ReadAt, F: FnMut(usize, &[u8]) -> ControlFlow<()>>(
    c: &Container<R>,
    p: &Page,
    lim: &Limits,
    pool: Option<&ThreadPool>,
    cancel: &AtomicBool,
    probe: bool,
    f: &mut F,
) -> (Result<(), Error>, Probe) {
    let ledger = Ledger::new(lim.max_alloc);
    let gauge = Gauge::new();
    let cx = Cx {
        c,
        p,
        lim,
        pool,
        cancel,
        ledger: &ledger,
        gauge: &gauge,
        probe,
    };
    let r = run(&cx, f);
    let probe = Probe {
        charged_peak: ledger.peak(),
        threads_peak: gauge.peak(),
    };
    (r, probe)
}

fn run<R: ReadAt, F: FnMut(usize, &[u8]) -> ControlFlow<()>>(
    cx: &Cx<'_, R>,
    f: &mut F,
) -> Result<(), Error> {
    let rb = cx.p.row_bytes();
    let band = cx.lim.max_band_bytes;
    if rb > band {
        return Err(limit("band bytes", rb, band));
    }
    let (mut from, mut par) = (0, true);
    loop {
        let next = if cx.p.tile_w > 0 {
            tiles(cx, f, from, par)?
        } else {
            strips(cx, f, from, par)?
        };
        match next {
            Next::Done => return Ok(()),
            // The serial loop, from where a worker failed.
            Next::Retry(i) => (from, par) = (i, false),
        }
    }
}

/// The workers of a pass over `chunks` chunks and the pool they run on, or `None` to run on the
/// calling thread. A worker holds `per` ledger bytes at most (`held` of them already booked by
/// the caller), and the shared pool starts only once there is a pass worth threading.
fn plan<'a, R: ReadAt>(
    cx: &Cx<'a, R>,
    chunks: u64,
    per: u64,
    held: u64,
) -> Option<(&'a ThreadPool, usize)> {
    let t = cx.lim.max_threads.max(1);
    if t < 2 || chunks < 2 {
        return None;
    }
    let fit = cx.ledger.left().saturating_add(held) / per.max(1);
    let k = t
        .min(usize::try_from(chunks).unwrap_or(usize::MAX))
        .min(usize::try_from(fit).unwrap_or(usize::MAX));
    if k < 2 {
        return None;
    }
    let pool = match cx.pool {
        Some(p) => p,
        None => shared_pool()?,
    };
    let k = k.min(pool.threads() + 1);
    (k >= 2).then_some((pool, k))
}

/// The largest compressed chunk among the first `n` (at most 64), for the ledger's estimate of a
/// JPEG worker; the other codecs read their chunk as a stream.
fn chunk_estimate<R: ReadAt>(cx: &Cx<'_, R>, n: usize) -> u64 {
    if cx.p.compression != JPEG {
        return 0;
    }
    (0..cx.p.chunks().min(n.min(64) as u64))
        .filter_map(|i| cx.c.chunk(&cx.p.offsets, &cx.p.counts, i).ok())
        .map(|(_, n)| n as u64)
        .max()
        .unwrap_or(0)
}

/// Strips, one band per strip when a strip fits the band buffer, else a strip in several bands.
fn strips<R: ReadAt, F: FnMut(usize, &[u8]) -> ControlFlow<()>>(
    cx: &Cx<'_, R>,
    f: &mut F,
    from: usize,
    par: bool,
) -> Result<Next, Error> {
    let (c, p, cancel) = (cx.c, cx.p, cx.cancel);
    let band = cx.lim.max_band_bytes;
    let rb = p.row_bytes();
    let mut env = serial_env(cx);
    let mut left = cx.lim.max_alloc;
    let buf_rows = (p.rows_per_strip as u64).min(band / rb);
    let mut buf = env.alloc(buf_rows * rb, &mut left)?;
    let rbu = rb as usize;
    let rps = p.rows_per_strip;
    // A strip that fits the band buffer is one band, so its rows can be decoded apart from the
    // others and handed over whole.
    if par && rps as u64 == buf_rows {
        let size = buf.len() as u64;
        let nested = nested_need(
            p.compression,
            p.width,
            jpeg_bytes(band, p.width, rps),
            chunk_estimate(cx, cx.lim.max_threads),
            size,
            left,
        );
        if let Some((pool, k)) = plan(cx, p.chunks(), size + nested, size) {
            let job = |i: usize, b: &mut [u8], env: &mut Env<'_>| -> Result<(), Error> {
                let y0 = i * rps;
                let rows = rps.min(p.height - y0);
                let (off, n) = c.chunk(&p.offsets, &p.counts, i as u64)?;
                let prm = params(
                    p,
                    p.width,
                    rows,
                    band,
                    jpeg_bytes(band, p.width, rows),
                    left,
                );
                let mut rr = codec::new(c.section(off, n), &prm, cancel, env)?;
                fill_rows(&mut rr, c.bo, p, p.width, rows, rbu, b)
            };
            let deliver = |i: usize, b: &[u8]| -> Result<(), Error> {
                check(cancel)?;
                let y0 = i * rps;
                let rows = rps.min(p.height - y0);
                if f(y0, &b[..rows * rbu]).is_break() {
                    return Err(Error::Stopped);
                }
                Ok(())
            };
            return pipeline(
                cx,
                pool,
                k,
                p.chunks() as usize,
                size,
                Some(buf),
                job,
                deliver,
            );
        }
    }
    for s in from as u64..p.chunks() {
        env.book_buffers();
        let y0 = s as usize * p.rows_per_strip;
        let rows = p.rows_per_strip.min(p.height - y0);
        let (off, n) = c.chunk(&p.offsets, &p.counts, s)?;
        let mut rr = codec::new(
            c.section(off, n),
            &params(p, p.width, rows, band, band, left),
            cancel,
            &mut env,
        )?;
        let mut done = 0;
        while done < rows {
            check(cancel)?;
            let k = (rows - done).min(buf.len() / rbu);
            if p.compression == JPEG {
                rr.read_rows(&mut buf[..k * rbu], rbu)?;
            } else {
                for r in 0..k {
                    let row = &mut buf[r * rbu..(r + 1) * rbu];
                    rr.read_row(row)?;
                    fix(c.bo, p, row, p.width);
                }
            }
            if f(y0 + done, &buf[..k * rbu]).is_break() {
                return Err(Error::Stopped);
            }
            done += k;
        }
    }
    Ok(Next::Done)
}

/// Decodes one tile row at a time: every tile of the row, row by row, into a buffer of the
/// tile row's full-width rows. A tile row bigger than `max_band_bytes` is refused.
fn tiles<R: ReadAt, F: FnMut(usize, &[u8]) -> ControlFlow<()>>(
    cx: &Cx<'_, R>,
    f: &mut F,
    from: usize,
    par: bool,
) -> Result<Next, Error> {
    let (c, p, cancel) = (cx.c, cx.p, cx.cancel);
    let band = cx.lim.max_band_bytes;
    let rb = p.row_bytes();
    let trb = (p.tile_w as u64 * p.spp as u64 * p.bps as u64).div_ceil(8);
    let got = p.tile_l.min(p.height) as u64 * rb;
    if got > band {
        return Err(limit("band bytes", got, band));
    }
    if trb > band {
        return Err(limit("band bytes", trb, band));
    }
    let mut env = serial_env(cx);
    let mut left = cx.lim.max_alloc;
    let mut buf = env.alloc(got, &mut left)?;
    let mut tmp = env.alloc(trb, &mut left)?;
    let (rbu, trbu) = (rb as usize, trb as usize);
    let across = p.across as usize;
    if par {
        // A worker decodes a whole tile into a buffer of its own.
        let size = p.tile_l.min(p.height) as u64 * trb;
        let nested = nested_need(
            p.compression,
            p.tile_w,
            jpeg_bytes(band, p.tile_w, p.tile_l),
            chunk_estimate(cx, cx.lim.max_threads),
            size,
            left,
        );
        if let Some((pool, k)) = plan(cx, p.chunks(), size + nested, 0) {
            let job = |i: usize, b: &mut [u8], env: &mut Env<'_>| -> Result<(), Error> {
                let y0 = (i / across) * p.tile_l;
                let rows = p.tile_l.min(p.height - y0);
                let (off, n) = c.chunk(&p.offsets, &p.counts, i as u64)?;
                let prm = params(
                    p,
                    p.tile_w,
                    p.tile_l,
                    band,
                    jpeg_bytes(band, p.tile_w, p.tile_l),
                    left,
                );
                let mut rr = codec::new(c.section(off, n), &prm, cancel, env)?;
                fill_rows(&mut rr, c.bo, p, p.tile_w, rows, trbu, b)
            };
            let deliver = |i: usize, b: &[u8]| -> Result<(), Error> {
                check(cancel)?;
                let (ty, tx) = (i / across, i % across);
                let y0 = ty * p.tile_l;
                let rows = p.tile_l.min(p.height - y0);
                let xoff = tx * trbu;
                for r in 0..rows {
                    let dst = &mut buf[r * rbu + xoff..(r + 1) * rbu];
                    let n = dst.len().min(trbu);
                    dst[..n].copy_from_slice(&b[r * trbu..r * trbu + n]);
                }
                if tx + 1 == across && f(y0, &buf[..rows * rbu]).is_break() {
                    return Err(Error::Stopped);
                }
                Ok(())
            };
            return match pipeline(cx, pool, k, p.chunks() as usize, size, None, job, deliver)? {
                Next::Retry(i) => Ok(Next::Retry(i / across)),
                Next::Done => Ok(Next::Done),
            };
        }
    }
    for ty in from as u64..p.down {
        let y0 = ty as usize * p.tile_l;
        let rows = p.tile_l.min(p.height - y0);
        for tx in 0..p.across {
            check(cancel)?;
            env.book_buffers();
            let (off, n) = c.chunk(&p.offsets, &p.counts, ty * p.across + tx)?;
            let mut rr = codec::new(
                c.section(off, n),
                &params(p, p.tile_w, p.tile_l, band, band, left),
                cancel,
                &mut env,
            )?;
            // tile widths are byte-aligned (checked at parse)
            let xoff = tx as usize * trbu;
            for r in 0..rows {
                rr.read_row(&mut tmp)?;
                fix(c.bo, p, &mut tmp, p.tile_w);
                let dst = &mut buf[r * rbu + xoff..(r + 1) * rbu];
                let n = dst.len().min(tmp.len());
                dst[..n].copy_from_slice(&tmp[..n]);
            }
        }
        if f(y0, &buf[..rows * rbu]).is_break() {
            return Err(Error::Stopped);
        }
    }
    Ok(Next::Done)
}

/// The chunks in flight: claimed, decoded and not yet delivered.
struct State<'a> {
    /// The next chunk to claim.
    next: usize,
    /// The next chunk the caller delivers.
    deliver: usize,
    stop: bool,
    /// The pool tasks that have not returned (queued ones included).
    active: usize,
    /// Decoded chunks by index; `None` is a chunk whose job failed.
    done: BTreeMap<usize, Option<Buf<'a>>>,
    /// Delivered buffers, reused before a new one is booked.
    free: Vec<Buf<'a>>,
}

impl<'a> State<'a> {
    /// Claims the next chunk, with a buffer to decode it into if one is free, unless the window
    /// of `w` undelivered chunks is full or there is none left.
    fn claim(&mut self, n: usize, w: usize) -> Option<(usize, Option<Buf<'a>>)> {
        if self.stop || self.next >= n || self.next >= self.deliver + w {
            return None;
        }
        self.next += 1;
        Some((self.next - 1, self.free.pop()))
    }
}

struct Pipe<'a> {
    st: Mutex<State<'a>>,
    cv: Condvar,
}

/// A mutex's data even if a job panicked while holding it (the panic is re-raised by the scope).
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl<'a> Pipe<'a> {
    fn wait<'g>(&self, g: MutexGuard<'g, State<'a>>) -> MutexGuard<'g, State<'a>> {
        self.cv.wait(g).unwrap_or_else(|e| e.into_inner())
    }

    fn post(&self, i: usize, r: Option<Buf<'a>>) {
        lock(&self.st).done.insert(i, r);
        self.cv.notify_all();
    }
}

/// Posts a failed chunk if its worker unwinds, so the caller never waits for it.
struct Claim<'p, 'a> {
    pipe: &'p Pipe<'a>,
    i: usize,
    posted: bool,
}

impl Drop for Claim<'_, '_> {
    fn drop(&mut self) {
        if !self.posted {
            self.pipe.post(self.i, None);
        }
    }
}

/// Stops the workers when the caller leaves its loop, however it leaves.
struct StopOnDrop<'p, 'a>(&'p Pipe<'a>);

impl Drop for StopOnDrop<'_, '_> {
    fn drop(&mut self) {
        lock(&self.0.st).stop = true;
    }
}

/// Decodes chunks `0..n` with `job` on `k` threads, the caller and up to `k - 1` tasks of `pool`,
/// and has the caller `deliver` them in order. Workers claim chunks in order, and no more than `k`
/// are claimed and not yet delivered, so the buffers (`size` bytes each, booked in the ledger)
/// stay `k` at most. The caller decodes a chunk itself whenever the next one is not ready.
/// A pool task never waits: when the window is full it returns, and the caller, after each
/// delivery, starts tasks again up to `k - 1`. So a callback that itself runs a pipeline on the
/// same pool cannot leave it without a free thread.
/// Returns [`Next::Retry`] at the first chunk whose job failed, after the workers have stopped
/// and everything they held is released; the caller's own errors (`deliver`'s) come back as they
/// are.
fn pipeline<'a, R: ReadAt, G, D>(
    cx: &Cx<'a, R>,
    pool: &ThreadPool,
    k: usize,
    n: usize,
    size: u64,
    seed: Option<Buf<'a>>,
    job: G,
    mut deliver: D,
) -> Result<Next, Error>
where
    G: Fn(usize, &mut [u8], &mut Env<'a>) -> Result<(), Error> + Sync,
    D: FnMut(usize, &[u8]) -> Result<(), Error>,
{
    let pipe = Pipe {
        st: Mutex::new(State {
            next: 0,
            deliver: 0,
            stop: false,
            active: 0,
            done: BTreeMap::new(),
            free: seed.into_iter().collect(),
        }),
        cv: Condvar::new(),
    };
    // One chunk: a free buffer or a new one, filled by the job; `None` for any failure.
    let decode = |i: usize, buf: Option<Buf<'a>>, env: &mut Env<'a>| -> Option<Buf<'a>> {
        let b = match buf {
            Some(b) => Some(b),
            None => env.alloc_held(size).ok(),
        };
        let r = b.and_then(|mut b| job(i, &mut b[..], env).ok().map(|()| b));
        // The buffers the job left in the environment are held until this thread's next chunk:
        // book them, as they are live beside the other threads' chunks.
        env.book_buffers();
        r
    };
    let work = || {
        // Nested JPEG calls run on this thread alone: the call's threads are the workers.
        let mut env = Env::new(cx.ledger, 1);
        let mut live = None;
        loop {
            let claimed = lock(&pipe.st).claim(n, k);
            let Some((i, buf)) = claimed else { break };
            if live.is_none() {
                live = Some(Live::enter(cx.gauge));
            }
            let mut claim = Claim {
                pipe: &pipe,
                i,
                posted: false,
            };
            let r = decode(i, buf, &mut env);
            claim.posted = true;
            pipe.post(i, r);
        }
        // Off the gauge and out of the ledger before the slot is given up, so the caller's next
        // task cannot run beside this one: the peak stays within `k`.
        drop(live);
        drop(env);
        lock(&pipe.st).active -= 1;
    };
    let work = &work;
    let mut env = Env::new(cx.ledger, 1);
    pool.scope(|s| -> Result<Next, Error> {
        // Starts tasks up to `k - 1`, as many as there are chunks to claim in the window.
        let top_up = || {
            let add = {
                let mut g = lock(&pipe.st);
                let open = n.min(g.deliver + k).saturating_sub(g.next);
                let add = if g.stop {
                    0
                } else {
                    (k - 1).saturating_sub(g.active).min(open)
                };
                g.active += add;
                add
            };
            for _ in 0..add {
                s.spawn(move |_| work());
            }
        };
        let _stop = StopOnDrop(&pipe);
        top_up();
        for i in 0..n {
            let got = {
                let mut g = lock(&pipe.st);
                loop {
                    if let Some(r) = g.done.remove(&i) {
                        break r;
                    }
                    if let Some((j, buf)) = g.claim(n, k) {
                        drop(g);
                        let r = decode(j, buf, &mut env);
                        pipe.post(j, r);
                        g = lock(&pipe.st);
                        continue;
                    }
                    g = pipe.wait(g);
                }
            };
            let Some(buf) = got else {
                return Ok(Next::Retry(i));
            };
            deliver(i, &buf[..])?;
            let mut g = lock(&pipe.st);
            g.free.push(buf);
            g.deliver = i + 1;
            drop(g);
            top_up();
        }
        Ok(Next::Done)
    })
}
