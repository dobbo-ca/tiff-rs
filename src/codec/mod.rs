//! Turns a strip's or tile's compressed bytes into packed rows, one row at a time, so no decoder
//! ever holds more than a row (JPEG: a band). Ported from tiff-go's `internal/codec`.

mod ccitt;
mod ccitt_tables;
mod jpeg4;
mod jpeg_rows;
mod lzw;
mod packbits;

use crate::error::{Error, corrupt, limit, unsupported};
use crate::ledger::{Buf, Held, Ledger};
use crate::page::{CCITT3, CCITT4, DEFLATE, DEFLATE_X, JPEG, LZW, NONE, PACKBITS};
use crate::par::{Gauge, ThreadPool};
use crate::readat::{ReadAt, SectionReader};
use std::io::{self, BufRead, Read};
use std::sync::atomic::{AtomicBool, Ordering};

/// Yields the chunk's rows in order, `dst.len()` bytes each.
pub(crate) trait RowReader {
    fn read_row(&mut self, dst: &mut [u8]) -> Result<(), Error>;

    /// `read_row` for each `rb`-byte row of `dst`, in order.
    fn read_rows(&mut self, dst: &mut [u8], rb: usize) -> Result<(), Error> {
        for row in dst.chunks_exact_mut(rb) {
            self.read_row(row)?;
        }
        Ok(())
    }
}

/// Describes one strip or tile.
pub(crate) struct Params<'a> {
    pub(crate) compression: u64,
    pub(crate) t4: u64,
    pub(crate) spp: usize,
    pub(crate) bps: usize,
    /// The chunk's frame: tile size, or strip width and rows.
    pub(crate) w: usize,
    pub(crate) h: usize,
    pub(crate) jpeg_tables: &'a [u8],
    /// PhotometricInterpretation is RGB: three JPEG samples are RGB as stored, decoded with no
    /// colour transform whatever the stream's markers say (libtiff, OpenSlide; tiff-go reads the
    /// markers). YCbCr keeps the markers' reading.
    pub(crate) jpeg_rgb: bool,
    pub(crate) band_bytes: u64,
    /// The decoded bytes a JPEG call is budgeted for: `band_bytes` when the chunk streams, the
    /// chunk's own decoded size (at most `band_bytes`) when it is decoded whole on a worker.
    pub(crate) jpeg_bytes: u64,
    /// What the codec may still allocate as a serial call sees it (`Limits::max_alloc` less what
    /// the caller holds). Every limit error is judged against it, so a worker refuses what the
    /// serial path would refuse; the ledger books what is really held.
    pub(crate) alloc_left: u64,
}

/// What a Deflate reader may hold for as long as it lives, booked in the ledger: its window and
/// one step of output, about 100 KiB (deflate-rs documents it), with room to spare.
const DEFLATE_RESERVE: u64 = 1 << 20;

/// What a worker decoding one chunk of `chunk_bytes` compressed bytes holds in the ledger beside
/// its output buffer, as far as it can be told before the chunk is read: the chunk itself and the
/// nested call's reservation (`jpeg_bytes` is the decoded size a JPEG call is budgeted for,
/// `decoded` the size of the output buffer, which a Deflate chunk is decoded into a second time,
/// `left` what a serial call has left for it).
pub(crate) fn nested_need(
    compression: u64,
    w: usize,
    jpeg_bytes: u64,
    chunk_bytes: u64,
    decoded: u64,
    left: u64,
) -> u64 {
    match compression {
        JPEG => jpeg_rows::jpeg_budget(jpeg_bytes).saturating_add(chunk_bytes),
        DEFLATE | DEFLATE_X => DEFLATE_RESERVE.min(left).saturating_add(decoded),
        CCITT3 | CCITT4 => 2 * w as u64,
        _ => 0,
    }
}

/// The most bytes read from a chunk between two cancel checks.
const READ_BLOCK: usize = 1 << 20;

/// The size of the read buffer of every codec that reads its chunk as a stream.
const READ_BUF: usize = 4096;

/// What a thread keeps from chunk to chunk, and a [`crate::Decoder`] from call to call: the JPEG
/// decoder and the Deflate decoder with their tables, and the buffers every codec would otherwise
/// allocate for each chunk. A chunk no larger than an earlier one, in compressed and in decoded
/// bytes, allocates nothing. Each use books in the ledger of its call the capacity the buffer
/// keeps or what the chunk needs, whichever is more (a buffer whose capacity does not fit is freed
/// first). Not booked: the LZW table, the read buffer and the Deflate decoder's tables, which are
/// fixed in size.
pub(crate) struct State {
    dec: jpeg::Decoder,
    /// The reservation the decoder's buffers were retained under, 0 for a fresh one.
    dec_cap: u64,
    z: deflate::zlib::Decoder,
    /// The compressed bytes of a JPEG chunk, or of a Deflate chunk decoded whole.
    src: Vec<u8>,
    /// A Deflate chunk decoded whole.
    out: Vec<u8>,
    /// The read buffer of the stream codecs, `READ_BUF` bytes once it is used.
    rbuf: Vec<u8>,
    /// The band copy of a JPEG chunk.
    jrows: Vec<u8>,
    /// The two rows of a CCITT chunk.
    cc: (Vec<u8>, Vec<u8>),
    lzw: Option<Box<lzw::Tables>>,
}

impl State {
    pub(crate) fn new() -> Self {
        Self {
            dec: jpeg::Decoder::new(),
            dec_cap: 0,
            z: deflate::zlib::Decoder::new(),
            src: Vec::new(),
            out: Vec::new(),
            rbuf: Vec::new(),
            jrows: Vec::new(),
            cc: (Vec::new(), Vec::new()),
            lzw: None,
        }
    }
}

/// The state one thread keeps from chunk to chunk: the ledger it books against and a [`State`]
/// whose JPEG decoder has its retained buffers booked as a reservation. The decoder is a
/// releasable cache: when a charge would fail while it holds a reservation, it is replaced by a
/// fresh one and the reservation is released, so a call fails exactly when it would without the
/// cache.
pub(crate) struct Env<'l> {
    pub(crate) ledger: &'l Ledger,
    /// `Limits::max_threads` of the nested jpeg-rs calls.
    pub(crate) jpeg_threads: usize,
    pub(crate) st: State,
    /// The caller's jpeg-rs threads for `dec` and the probe decodes.
    pub(crate) jpeg_pool: Option<jpeg::ThreadPool>,
    /// Set in probe mode: the gauge that nested jpeg-rs thread peaks are noted in. A probe call
    /// decodes through `jpeg::decode_probe`, which reports them, instead of through `dec`.
    pub(crate) probe: Option<&'l Gauge>,
    dec_held: Option<Held<'l>>,
    /// What `book_retained` booked for the buffers of `st` that the call has not used yet. A buffer
    /// gives its share back when the call takes it.
    idle: Option<Held<'l>>,
}

impl<'l> Env<'l> {
    pub(crate) fn new(ledger: &'l Ledger, jpeg_threads: usize) -> Self {
        Self::with_state(ledger, jpeg_threads, State::new())
    }

    /// An environment over the state a thread (or a [`crate::Decoder`]) kept from before. The
    /// decoder's retained buffers are not booked until the call reserves it.
    pub(crate) fn with_state(ledger: &'l Ledger, jpeg_threads: usize, st: State) -> Self {
        Self {
            ledger,
            jpeg_threads,
            st,
            jpeg_pool: None,
            probe: None,
            dec_held: None,
            idle: None,
        }
    }

    /// Books what `st` kept from earlier calls, so that the call's peak counts it: the decoder
    /// under its reservation, and the buffers by their capacity. What does not fit is freed.
    pub(crate) fn book_retained(&mut self) {
        if self.st.dec_cap > 0 {
            match self.ledger.charge(self.st.dec_cap) {
                Ok(h) => self.dec_held = Some(h),
                Err(_) => {
                    self.st.dec = self.fresh();
                    self.st.dec_cap = 0;
                }
            }
        }
        self.book_buffers();
    }

    /// Books the buffers `st` keeps by their capacity, in place of any earlier booking of them
    /// (the call is between two chunks, and holds none of them). What does not fit is freed.
    pub(crate) fn book_buffers(&mut self) {
        self.idle = None;
        let st = &self.st;
        let caps = [
            st.src.capacity(),
            st.out.capacity(),
            st.jrows.capacity(),
            st.cc.0.capacity(),
            st.cc.1.capacity(),
        ];
        let total: u64 = caps.iter().map(|&c| c as u64).sum();
        if total == 0 {
            return;
        }
        match self.ledger.charge(total) {
            Ok(h) => self.idle = Some(h),
            Err(_) => {
                self.st.src = Vec::new();
                self.st.out = Vec::new();
                self.st.jrows = Vec::new();
                self.st.cc = (Vec::new(), Vec::new());
            }
        }
    }

    /// The call takes buffers of `n` bytes of capacity from `st`: their share of the booking
    /// `book_retained` made goes back.
    pub(crate) fn release_idle(&mut self, n: usize) {
        if let Some(h) = self.idle.as_mut() {
            h.release(n as u64);
        }
    }

    /// Books the vector `v` of the state for a chunk of `n` bytes: its capacity if that is more,
    /// else `n`. A capacity that does not fit is freed, and `n` is booked.
    fn charge_kept(&mut self, v: &mut Vec<u8>, n: u64) -> Result<Held<'l>, Error> {
        self.release_idle(v.capacity());
        let cap = v.capacity() as u64;
        if cap > n {
            match self.ledger.charge(cap) {
                Ok(h) => return Ok(h),
                Err(_) => *v = Vec::new(),
            }
        }
        self.charge(n)
    }

    /// The state, to keep for the next call.
    pub(crate) fn into_state(self) -> State {
        self.st
    }

    /// Hands the caller's pool to the nested jpeg-rs calls, and sets the probe gauge.
    pub(crate) fn with_jpeg(mut self, pool: Option<&ThreadPool>, probe: Option<&'l Gauge>) -> Self {
        self.jpeg_pool = pool.and_then(ThreadPool::jpeg).cloned();
        self.probe = probe;
        self.st.dec = self.fresh();
        self
    }

    /// An empty jpeg-rs decoder on the caller's pool, if it gave one.
    fn fresh(&self) -> jpeg::Decoder {
        match &self.jpeg_pool {
            Some(p) => jpeg::Decoder::with_pool(p.clone()),
            None => jpeg::Decoder::new(),
        }
    }

    /// Books `n` bytes, dropping the caches first if that is what makes room: the decoder, and the
    /// buffers `book_retained` booked that the call has not taken.
    pub(crate) fn charge(&mut self, n: u64) -> Result<Held<'l>, Error> {
        let r = self.ledger.charge(n);
        if r.is_err() && self.drop_caches() {
            return self.ledger.charge(n);
        }
        r
    }

    /// Frees the caches the call holds booked: the decoder (replaced by a fresh one) and the idle
    /// buffers. Reports whether there was any, that is, whether a booking is worth trying again.
    pub(crate) fn drop_caches(&mut self) -> bool {
        let had = self.dec_held.is_some() || self.idle.is_some();
        if self.dec_held.take().is_some() {
            self.st.dec = self.fresh();
            self.st.dec_cap = 0;
        }
        if self.idle.take().is_some() {
            self.st.src = Vec::new();
            self.st.out = Vec::new();
            self.st.jrows = Vec::new();
            self.st.cc = (Vec::new(), Vec::new());
        }
        had
    }

    /// `n` zero bytes booked in the ledger, with no serial judgement: a worker's own buffer.
    pub(crate) fn alloc_held(&mut self, n: u64) -> Result<Buf<'l>, Error> {
        let u = usize::try_from(n).map_err(|_| Error::OutOfMemory)?;
        Buf::new(u, self.charge(n)?)
    }

    /// `n` zero bytes, judged against `left` (which it reduces) as a serial call would and booked
    /// in the ledger.
    pub(crate) fn alloc(&mut self, n: u64, left: &mut u64) -> Result<Buf<'l>, Error> {
        let u = fit(n, left)?;
        Buf::new(u, self.charge(n)?)
    }

    /// The whole chunk, in blocks of at most 1 MiB with a cancel check between them (a chunk of one
    /// block is read without one, as a plain read would).
    pub(crate) fn read_all<R: ReadAt>(
        &mut self,
        mut src: SectionReader<'_, R>,
        left: &mut u64,
        cancel: &AtomicBool,
    ) -> Result<Buf<'l>, Error> {
        let mut b = self.alloc(src.len(), left)?;
        for (i, blk) in b.chunks_mut(READ_BLOCK).enumerate() {
            if i > 0 {
                check(cancel)?;
            }
            src.read_exact(blk).map_err(read_err)?;
        }
        Ok(b)
    }

    /// As `read_all`, into the vector the state keeps: it comes back apart from its charge, and
    /// goes back to the state with `give_src` (or with the reader that holds it).
    pub(crate) fn read_chunk<R: ReadAt>(
        &mut self,
        mut src: SectionReader<'_, R>,
        left: &mut u64,
        cancel: &AtomicBool,
    ) -> Result<(Vec<u8>, Held<'l>), Error> {
        let n = src.len();
        let u = fit(n, left)?;
        let mut v = std::mem::take(&mut self.st.src);
        let held = self.charge_kept(&mut v, n)?;
        v.clear();
        v.try_reserve_exact(u).map_err(|_| Error::OutOfMemory)?;
        v.resize(u, 0);
        for (i, blk) in v.chunks_mut(READ_BLOCK).enumerate() {
            if i > 0 {
                check(cancel)?;
            }
            src.read_exact(blk).map_err(read_err)?;
        }
        Ok((v, held))
    }

    /// Hands back the vector of `read_chunk`.
    pub(crate) fn give_src(&mut self, v: Vec<u8>) {
        self.st.src = v;
    }

    /// Books `real` bytes for a JPEG call and for the buffers the decoder keeps after it, or what
    /// the decoder already keeps if that is more, after the serial check that `virt` fits `left`.
    /// The hold the decoder has is raised, never released first, so its buffers are never live
    /// unbooked. If the ledger has no room for the raise, the decoder is replaced, the hold is
    /// released and `real` is booked, as for a decoder that was never kept.
    pub(crate) fn reserve_decoder(&mut self, virt: u64, left: u64, real: u64) -> Result<(), Error> {
        if virt > left {
            return Err(limit("alloc", virt, left));
        }
        let want = real.max(self.st.dec_cap);
        let raised = if let Some(h) = self.dec_held.as_mut() {
            h.raise(want)
        } else {
            match self.ledger.charge(want) {
                Ok(h) => {
                    self.dec_held = Some(h);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        };
        if raised.is_ok() {
            self.st.dec_cap = want;
            return Ok(());
        }
        self.st.dec = self.fresh();
        self.dec_held = None;
        self.st.dec_cap = 0;
        self.dec_held = Some(self.charge(real)?);
        self.st.dec_cap = real;
        Ok(())
    }
}

pub(crate) fn check(cancel: &AtomicBool) -> Result<(), Error> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    Ok(())
}

/// Judges `n` bytes against `left` and takes them off it.
fn fit(n: u64, left: &mut u64) -> Result<usize, Error> {
    if n > *left {
        return Err(limit("alloc", n, *left));
    }
    let u = usize::try_from(n).map_err(|_| Error::OutOfMemory)?;
    *left -= n;
    Ok(u)
}

/// `n` zero bytes in `v`, which keeps its capacity from call to call, judged against `left`
/// (which it reduces) and booked in `l`: the charge comes back. It is the capacity of `v` if
/// that is more than `n`; a capacity that does not fit is freed.
pub(crate) fn alloc_in<'l>(
    v: &mut Vec<u8>,
    n: u64,
    left: &mut u64,
    l: &'l Ledger,
) -> Result<Held<'l>, Error> {
    let u = fit(n, left)?;
    let cap = v.capacity() as u64;
    let held = match l.charge(cap.max(n)) {
        Ok(h) => h,
        Err(_) if cap > n => {
            *v = Vec::new();
            l.charge(n)?
        }
        Err(e) => return Err(e),
    };
    v.clear();
    v.try_reserve_exact(u).map_err(|_| Error::OutOfMemory)?;
    v.resize(u, 0);
    Ok(held)
}

/// Maps what a deflate-rs call refused or found.
fn deflate_err(d: &deflate::Error) -> Error {
    match d {
        deflate::Error::Cancelled => Error::Cancelled,
        deflate::Error::AllocLimit => limit("deflate alloc", 0, 0),
        deflate::Error::OutputLimit => limit("deflate output", 0, 0),
        deflate::Error::OutOfMemory => Error::OutOfMemory,
        deflate::Error::Io(k) => Error::Io(*k),
        d => corrupt(d.to_string()),
    }
}

/// Maps an error of a `Read` over a chunk: an end of data is corruption, our own errors and
/// deflate-rs's come back as they are.
pub(crate) fn read_err(e: io::Error) -> Error {
    if e.kind() == io::ErrorKind::UnexpectedEof {
        return corrupt("truncated data");
    }
    if let Some(inner) = e.get_ref() {
        if let Some(d) = inner.downcast_ref::<deflate::Error>() {
            return deflate_err(d);
        }
        if let Some(t) = inner.downcast_ref::<Error>() {
            return t.clone();
        }
    }
    match e.kind() {
        io::ErrorKind::InvalidData => corrupt(e.to_string()),
        k => Error::Io(k),
    }
}

/// Rows from a byte stream (everything but CCITT and JPEG).
pub(crate) struct Stream<'a, R> {
    r: R,
    cancel: &'a AtomicBool,
    /// What the reader's state is booked as, until it drops.
    _held: Option<Held<'a>>,
}

impl<R: Read> RowReader for Stream<'_, R> {
    fn read_row(&mut self, dst: &mut [u8]) -> Result<(), Error> {
        check(self.cancel)?;
        self.r.read_exact(dst).map_err(read_err)
    }
}

fn stream<'a, R: Read>(r: R, cancel: &'a AtomicBool, held: Option<Held<'a>>) -> Stream<'a, R> {
    Stream {
        r,
        cancel,
        _held: held,
    }
}

/// A `BufReader` over a chunk that reads into a buffer of the caller's, so a chunk allocates no
/// read buffer. It makes the same reads of the source as `BufReader::with_capacity` of the
/// buffer's length: a refill reads the whole buffer, and a read as large as the buffer goes
/// straight to the source when the buffer is empty.
pub(crate) struct Buffered<'a, R> {
    src: SectionReader<'a, R>,
    buf: &'a mut [u8],
    pos: usize,
    cap: usize,
}

impl<R: ReadAt> Buffered<'_, R> {
    /// Out of line, so the per-byte `fill_buf` of the CCITT and LZW readers stays small.
    #[cold]
    #[inline(never)]
    fn refill(&mut self) -> io::Result<()> {
        self.cap = self.src.read(self.buf)?;
        self.pos = 0;
        Ok(())
    }
}

impl<R: ReadAt> Read for Buffered<'_, R> {
    #[inline]
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.pos == self.cap && out.len() >= self.buf.len() {
            return self.src.read(out);
        }
        let n = {
            let rem = self.fill_buf()?;
            let n = rem.len().min(out.len());
            out[..n].copy_from_slice(&rem[..n]);
            n
        };
        self.consume(n);
        Ok(n)
    }
}

impl<R: ReadAt> BufRead for Buffered<'_, R> {
    #[inline]
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.pos >= self.cap {
            self.refill()?;
        }
        // pos <= cap <= buf.len() always holds; `get` keeps a panic path out of the byte loop.
        Ok(self.buf.get(self.pos..self.cap).unwrap_or_default())
    }

    #[inline]
    fn consume(&mut self, n: usize) {
        self.pos = (self.pos + n).min(self.cap);
    }
}

/// The rows of one chunk, from whichever codec reads it. An enum, so that opening a chunk
/// allocates nothing; the JPEG frames that keep their own buffers (progressive, four components)
/// are boxed.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Rows<'a, 'l, R> {
    Raw(Stream<'a, Buffered<'a, R>>),
    PackBits(Stream<'a, packbits::PackBits<'a, Buffered<'a, R>>>),
    Lzw(Stream<'a, lzw::Lzw<'a, Buffered<'a, R>>>),
    Zlib(Stream<'a, deflate::zlib::Reader<'a, Buffered<'a, R>>>),
    /// A Deflate chunk decoded whole into the state's `out`.
    Whole(Stream<'a, &'a [u8]>),
    Ccitt(ccitt::Ccitt<'a, Buffered<'a, R>>),
    Jpeg(jpeg_rows::JpegRows<'a, 'l>),
    Boxed(Box<dyn RowReader + 'a>),
}

impl<R: ReadAt> RowReader for Rows<'_, '_, R> {
    fn read_row(&mut self, dst: &mut [u8]) -> Result<(), Error> {
        match self {
            Self::Raw(r) => r.read_row(dst),
            Self::PackBits(r) => r.read_row(dst),
            Self::Lzw(r) => r.read_row(dst),
            Self::Zlib(r) => r.read_row(dst),
            Self::Whole(r) => r.read_row(dst),
            Self::Ccitt(r) => r.read_row(dst),
            Self::Jpeg(r) => r.read_row(dst),
            Self::Boxed(r) => r.read_row(dst),
        }
    }

    fn read_rows(&mut self, dst: &mut [u8], rb: usize) -> Result<(), Error> {
        match self {
            Self::Raw(r) => r.read_rows(dst, rb),
            Self::PackBits(r) => r.read_rows(dst, rb),
            Self::Lzw(r) => r.read_rows(dst, rb),
            Self::Zlib(r) => r.read_rows(dst, rb),
            Self::Whole(r) => r.read_rows(dst, rb),
            Self::Ccitt(r) => r.read_rows(dst, rb),
            Self::Jpeg(r) => r.read_rows(dst, rb),
            Self::Boxed(r) => r.read_rows(dst, rb),
        }
    }
}

/// Decodes a whole Deflate chunk into `env.st.out` with the state's reusable decoder, when that
/// is certain to give what the streaming reader gives: the decoded size fits the band limit, the
/// serial budget has room for the compressed bytes, the decoded bytes and the reader's
/// reservation, the ledger can book them, the chunk reads in full and the stream decodes to its
/// last needed byte with no error. It returns what it booked, or `None` for every other case, and
/// the caller then runs the streaming reader over the chunk, which gives today's output and
/// today's errors; so a bad stream, a short chunk, a bad checksum, a cancel or a failing source
/// is never judged here. `decode_limit_into` stops after the bytes asked for and does not read
/// the Adler-32 there, as the reader that is not read on does not (deflate-rs tests it against
/// `io.LimitReader` over Go's reader, and fuzzes the reader against the whole-buffer decode).
fn inflate_whole<'l, R: ReadAt>(
    src: &SectionReader<'_, R>,
    p: &Params<'_>,
    cancel: &AtomicBool,
    env: &mut Env<'l>,
) -> Option<Held<'l>> {
    let rb = (p.w as u64)
        .checked_mul(p.spp as u64)?
        .checked_mul(p.bps as u64)?
        .div_ceil(8);
    let n_out = rb.checked_mul(p.h as u64)?;
    let n_in = src.len();
    let need = n_out.checked_add(n_in)?.checked_add(DEFLATE_RESERVE)?;
    if n_out == 0 || n_out > p.band_bytes || need > p.alloc_left || check(cancel).is_err() {
        return None;
    }
    let (out_len, in_len) = (usize::try_from(n_out).ok()?, usize::try_from(n_in).ok()?);
    // The buffers the state keeps are booked by their capacity, or freed if that does not fit.
    let (cap_in, cap_out) = (env.st.src.capacity() as u64, env.st.out.capacity() as u64);
    let big = cap_in
        .max(n_in)
        .saturating_add(cap_out.max(n_out))
        .saturating_add(DEFLATE_RESERVE);
    let held = match env.ledger.charge(big) {
        Ok(h) => h,
        Err(_) => {
            env.st.src = Vec::new();
            env.st.out = Vec::new();
            env.charge(need).ok()?
        }
    };
    let st = &mut env.st;
    st.src.clear();
    st.src.try_reserve_exact(in_len).ok()?;
    st.src.resize(in_len, 0);
    let mut s = src.clone();
    for (i, blk) in st.src.chunks_mut(READ_BLOCK).enumerate() {
        if i > 0 && check(cancel).is_err() {
            return None;
        }
        s.read_exact(blk).ok()?;
    }
    st.out.clear();
    st.out.try_reserve_exact(out_len).ok()?;
    let lim = deflate::Limits {
        max_output: usize::MAX,
        max_alloc: out_len,
    };
    st.z.decode_limit_into(&st.src, out_len, &mut st.out, &lim, cancel)
        .ok()?;
    (st.out.len() == out_len).then_some(held)
}

/// Opens a row reader over `src`, the chunk's compressed bytes.
pub(crate) fn new<'a, 'l: 'a, R: ReadAt>(
    src: SectionReader<'a, R>,
    p: &Params<'a>,
    cancel: &'a AtomicBool,
    env: &'a mut Env<'l>,
) -> Result<Rows<'a, 'l, R>, Error> {
    if p.compression == JPEG {
        return jpeg_rows::new(src, p, cancel, env);
    }
    if matches!(p.compression, DEFLATE | DEFLATE_X) {
        env.release_idle(env.st.src.capacity() + env.st.out.capacity());
        if let Some(held) = inflate_whole(&src, p, cancel, env) {
            return Ok(Rows::Whole(stream(&env.st.out[..], cancel, Some(held))));
        }
        // The streaming reader runs beside no buffer of the whole-chunk path.
        env.st.src = Vec::new();
        env.st.out = Vec::new();
    }
    // A reader's reservation and the rows of a CCITT chunk (booked on its first row) are booked
    // like any other call's: the caches go first if that is what makes room.
    let deflate_held = if matches!(p.compression, DEFLATE | DEFLATE_X) {
        Some(env.charge(DEFLATE_RESERVE.min(p.alloc_left))?)
    } else {
        None
    };
    if matches!(p.compression, CCITT3 | CCITT4) && env.ledger.left() < 2 * p.w as u64 {
        env.drop_caches();
    }
    let ledger = env.ledger;
    let State {
        rbuf,
        lzw: lzw_tables,
        cc,
        ..
    } = &mut env.st;
    if rbuf.len() != READ_BUF {
        rbuf.clear();
        rbuf.try_reserve_exact(READ_BUF)
            .map_err(|_| Error::OutOfMemory)?;
        rbuf.resize(READ_BUF, 0);
    }
    let br = Buffered {
        src,
        buf: &mut rbuf[..],
        pos: 0,
        cap: 0,
    };
    match p.compression {
        NONE => Ok(Rows::Raw(stream(br, cancel, None))),
        PACKBITS => Ok(Rows::PackBits(stream(
            packbits::PackBits::new(br, cancel),
            cancel,
            None,
        ))),
        LZW => {
            let t = &mut **lzw_tables.get_or_insert_with(lzw::Tables::new);
            Ok(Rows::Lzw(stream(
                lzw::Lzw::new(br, cancel, t)?,
                cancel,
                None,
            )))
        }
        DEFLATE | DEFLATE_X => {
            // Output is bounded by the rows asked for, so the reader's own cap stays open.
            let lim = deflate::Limits {
                max_output: usize::MAX,
                max_alloc: usize::try_from(p.alloc_left).unwrap_or(usize::MAX),
            };
            let z = deflate::zlib::Reader::new(br, &lim, cancel).map_err(|e| deflate_err(&e))?;
            Ok(Rows::Zlib(stream(z, cancel, deflate_held)))
        }
        CCITT3 | CCITT4 => {
            if p.spp != 1 || p.bps != 1 {
                return Err(unsupported(format!(
                    "ccitt with {} samples of {} bits",
                    p.spp, p.bps
                )));
            }
            // The two rows are booked by `alloc_in`, by their capacity.
            if let Some(h) = env.idle.as_mut() {
                h.release(cc.0.capacity() as u64 + cc.1.capacity() as u64);
            }
            // As libtiff, the decoded bits come back as they are, whatever the photometric.
            let group4 = p.compression == CCITT4;
            if !group4 && p.t4 & 5 != 0 {
                // 2-D coding, EOL fill bits: x/image/ccitt cannot read either
                return Err(unsupported(format!("group 3 options {}", p.t4)));
            }
            Ok(Rows::Ccitt(ccitt::Ccitt::new(
                br,
                group4,
                p.w,
                p.h,
                cancel,
                p.alloc_left,
                ledger,
                cc,
            )))
        }
        c => Err(unsupported(format!("compression {c}"))),
    }
}

/// One byte of `r`, or `None` at its end.
pub(crate) fn read_byte<R: BufRead>(r: &mut R) -> io::Result<Option<u8>> {
    loop {
        match r.fill_buf() {
            Ok(b) => {
                let Some(&v) = b.first() else {
                    return Ok(None);
                };
                r.consume(1);
                return Ok(Some(v));
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}
