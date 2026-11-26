//! Multi-page TIFF and BigTIFF decoding under strict resource limits.
//!
//! [`File::open`] walks the IFD chain (refusing loops and more than [`Limits::max_ifds`]
//! directories), [`File::page`] parses one directory (refusing more than
//! [`Limits::max_pixels`]), and [`Page::rows`] delivers the pixels as bands of full-width rows,
//! top to bottom. A strip or tile of any size is decoded incrementally, so no band exceeds
//! [`Limits::max_band_bytes`]; only a tile row (all tiles of one row) is buffered whole, and a
//! row of tiles larger than the cap is refused.
//!
//! Samples are delivered packed as stored, rows padded to a byte, 16-bit samples big-endian,
//! predictor undone. JPEG-compressed data comes back as 8-bit grey, RGB or four samples as stored
//! (CMYK). Three JPEG samples are RGB as stored, with no colour transform, when the
//! PhotometricInterpretation is RGB (as libtiff and OpenSlide read them; Aperio SVS), and are
//! converted as the stream's markers say (YCbCr unless an Adobe segment or ids R, G, B say RGB)
//! when it is YCbCr; an RGB page whose components are subsampled is unsupported, as in libtiff. The [`Info::photometric`] says how to read the samples;
//! [`Info::color_map`] carries a palette.
//!
//! Supported: classic and BigTIFF, either byte order; none, PackBits, LZW, Deflate, CCITT G3/G4
//! and JPEG compression; predictor 2 at 8 and 16 bits; 1 to 16 bits per sample; grey, RGB,
//! palette and CMYK. Everything else returns [`Error::Unsupported`].
//!
//! # Resource limits
//!
//! Every entry point takes [`Limits`] (through [`File::open`], which keeps them), and
//! [`Page::rows`] takes a cancel flag. Set them first:
//!
//! ```
//! use std::ops::ControlFlow;
//! use std::sync::atomic::AtomicBool;
//! use tiff::{File, Limits};
//!
//! # fn main() -> Result<(), tiff::Error> {
//! // A 2x2 8-bit grey TIFF, one strip.
//! let bytes: &[u8] = &[
//!     0x49, 0x49, 0x2a, 0x00, 0x0c, 0x00, 0x00, 0x00, 1, 2, 3, 4, 0x08, 0x00, 0x00, 0x01, 0x03,
//!     0x00, 0x01, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x01, 0x03, 0x00, 0x01, 0x00,
//!     0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x01, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0x08,
//!     0x00, 0x00, 0x00, 0x03, 0x01, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
//!     0x06, 0x01, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x11, 0x01, 0x04,
//!     0x00, 0x01, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x16, 0x01, 0x03, 0x00, 0x01, 0x00,
//!     0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x17, 0x01, 0x04, 0x00, 0x01, 0x00, 0x00, 0x00, 0x04,
//!     0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
//! ];
//! // At most one thread (no thread is spawned, no pool is touched) and 64 MiB for the whole call.
//! let limits = Limits {
//!     max_threads: 1,
//!     max_alloc: 64 << 20,
//!     ..Limits::default()
//! };
//! let file = File::open(bytes, bytes.len() as u64, &limits)?;
//! let page = file.page(0)?;
//! let cancel = AtomicBool::new(false);
//! let mut pixels = Vec::new();
//! page.rows(&cancel, |_y, band| {
//!     pixels.extend_from_slice(band);
//!     ControlFlow::Continue(())
//! })?;
//! assert_eq!(pixels, [1, 2, 3, 4]);
//!
//! // A page over `max_pixels` is refused from its header, before anything is allocated.
//! let tiny = Limits { max_pixels: 3, ..limits };
//! let file = File::open(bytes, bytes.len() as u64, &tiny)?;
//! assert!(matches!(file.page(0), Err(tiff::Error::Limit { .. })));
//! # Ok(())
//! # }
//! ```
//!
//! * `max_alloc` (default 1 GiB, bytes): the enforced peak of one call, all threads together.
//!   Every buffer that scales with the input is charged before it is allocated and released when
//!   it drops: the band buffers, a JPEG chunk and its decoder's planes, the Deflate window, and
//!   what the nested jpeg-rs and deflate-rs calls may use for as long as they live. Over it:
//!   [`Error::Limit`] named `alloc`. A call with less room runs on fewer threads, with the same
//!   output and the same errors.
//! * `max_threads` (default [`std::thread::available_parallelism`]): the most threads of one
//!   call, the calling thread included. 1 runs on the calling thread and never touches a pool;
//!   0 counts as 1. [`Page::rows`] decodes strips and tiles in parallel, and the output, the
//!   errors and the partial output are the same for every value, for a [`ReadAt`] whose result
//!   depends only on the offset and the length. The workers are the
//!   [`ThreadPool`] given to [`File::with_pool`], else the crate's lazy shared pool, started by the
//!   first call that has several chunks to decode and room for two workers; a call uses at most
//!   one more thread than the pool has. A pool's thread stacks are the pool's, not charged to
//!   `max_alloc`. A [`ThreadPool`] owns two sets of threads of the same count, tiff's and
//!   jpeg-rs's: a call that runs serially (one strip or tile, or a strip split in bands) hands
//!   the caller's pool to jpeg-rs, whose parallel passes then use `max_threads` threads of it.
//!   Without a pool, such a call uses jpeg-rs's own lazy shared pool (a second process-wide
//!   pool, started by a JPEG large enough to thread: a scan with restart markers and about 16 KiB
//!   of entropy data, or a frame of about 262 144 pixels of decoded region). When tiff runs several workers, nested
//!   jpeg-rs calls use one thread and no pool. The output and the errors do not depend on any of
//!   this, on the assumption that the [`ReadAt`] is as [`ReadAt`] says.
//! * `max_pixels` (default 178 956 970, pixels of one IFD): [`Error::Limit`] named `pixels`, from
//!   [`File::page`] and, for a chunk of more pixels, [`Page::decode_chunk_into`] and
//!   [`Page::raw_jpeg_into`]. `max_band_bytes` (default 64 MiB, decoded bytes buffered at once):
//!   `band bytes`. `max_ifds` (default 10 000, directories in the chain): `ifds`, from
//!   [`File::open`]. `max_metadata_bytes` (default 4 MiB, XMP bytes read per page) is no error:
//!   an XMP value over it is not read and its [`TagEntry`] says so.
//! * Cancellation: set the `AtomicBool` from any thread; the call returns [`Error::Cancelled`] at
//!   its next check (before every band, row, tile and megabyte read from a JPEG chunk).
//!
//! # Decoding one strip or tile
//!
//! [`Page::decode_chunk_into`] decodes a single strip or tile on the calling thread, for a caller
//! that runs its own workers. [`Page::layout`] says how the page is cut, and one [`Decoder`] per
//! worker keeps the codec state (the Huffman tables, the read and output buffers) from chunk to
//! chunk, so a chunk no larger than an earlier one, in compressed and in decoded bytes, allocates
//! nothing. Its limits are those of the [`File`]: `max_alloc` (the output and every codec buffer
//! except the fixed LZW table, read buffer and Deflate tables, charged to a ledger of the call
//! that also books what the `Decoder` kept from earlier calls), `max_pixels`, and
//! `max_band_bytes` for the size of the decoded chunk. See [`Decoder`] for an example.
//!
//! A Rust port of [dobbo-ca/tiff-go](https://github.com/dobbo-ca/tiff-go) at 756f16b.
#![forbid(unsafe_code)]
// The ported code keeps Go's index loops and long argument lists.
#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented
    )
)]

mod codec;
mod decode;
mod error;
mod ifd;
mod ledger;
mod page;
mod par;
mod rawjpeg;
mod readat;

pub use error::Error;
pub use page::TagEntry;
pub use par::{ThreadPool, shared_pool_started};
pub use readat::ReadAt;

use ifd::Container;
use std::fmt;
use std::ops::ControlFlow;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;

/// Default of [`Limits::max_ifds`]: 10 000 directories. Go's `Limits` took the defaults for zero
/// fields; here a zero field is a real cap.
pub const DEFAULT_MAX_IFDS: usize = 10_000;
/// Default of [`Limits::max_pixels`]: 178 956 970 pixels per IFD.
pub const DEFAULT_MAX_PIXELS: u64 = 178_956_970;
/// Default of [`Limits::max_band_bytes`]: 64 MiB.
pub const DEFAULT_MAX_BAND_BYTES: u64 = 64 << 20;
/// Default of [`Limits::max_metadata_bytes`]: 4 MiB.
pub const DEFAULT_MAX_METADATA_BYTES: u64 = 4 << 20;
/// Default of [`Limits::max_alloc`]: 1 GiB.
pub const DEFAULT_MAX_ALLOC: u64 = 1 << 30;

/// Caps what one [`File`] may cost. Every entry point takes them, directly or through the
/// `File` that holds them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Directories in the chain. Default [`DEFAULT_MAX_IFDS`] (10 000). More: [`Error::Limit`]
    /// named `ifds`, from [`File::open`].
    pub max_ifds: usize,
    /// Width times height of one IFD, in pixels. Default [`DEFAULT_MAX_PIXELS`] (178 956 970).
    /// More: [`Error::Limit`] named `pixels`, from [`File::page`], and from
    /// [`Page::decode_chunk_into`] and [`Page::raw_jpeg_into`] for a chunk of more pixels.
    pub max_pixels: u64,
    /// Decoded bytes buffered at once. Default [`DEFAULT_MAX_BAND_BYTES`] (64 MiB). A row, or a
    /// row of tiles, larger: [`Error::Limit`] named `band bytes`.
    pub max_band_bytes: u64,
    /// XMP bytes read per page, summed over its entries. Default
    /// [`DEFAULT_MAX_METADATA_BYTES`] (4 MiB). No error: an XMP value over it is not read and its
    /// [`TagEntry`] says so.
    pub max_metadata_bytes: u64,
    /// The peak working memory of one call, in bytes, all threads together: the band buffers, a
    /// JPEG chunk and its decoder's planes, the Deflate window. Default [`DEFAULT_MAX_ALLOC`]
    /// (1 GiB). Every buffer is charged before it is allocated and released when it drops. Over
    /// it: [`Error::Limit`] named `alloc`; the call then runs on fewer threads where it can, so
    /// the limit never depends on `max_threads`.
    pub max_alloc: u64,
    /// The most threads one call uses, the calling thread included. 1 runs on the calling thread
    /// and never touches a pool; 0 counts as 1. The default is
    /// [`std::thread::available_parallelism`], read once. No error. A call uses as many workers
    /// as it has strips or tiles to feed and memory for under `max_alloc`; the bands, the errors
    /// and any partial output are the same for every value (for a [`ReadAt`] as that trait
    /// says).
    pub max_threads: usize,
}

/// `available_parallelism`, read once: it reads cgroup files on every call.
pub(crate) fn default_threads() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_ifds: DEFAULT_MAX_IFDS,
            max_pixels: DEFAULT_MAX_PIXELS,
            max_band_bytes: DEFAULT_MAX_BAND_BYTES,
            max_metadata_bytes: DEFAULT_MAX_METADATA_BYTES,
            max_alloc: DEFAULT_MAX_ALLOC,
            max_threads: default_threads(),
        }
    }
}

// Photometric values of `Info::photometric`.
pub const WHITE_IS_ZERO: usize = page::WHITE_IS_ZERO as usize;
pub const BLACK_IS_ZERO: usize = page::BLACK_IS_ZERO as usize;
pub const RGB: usize = page::RGB as usize;
pub const PALETTE: usize = page::PALETTE as usize;
pub const CMYK: usize = page::CMYK as usize;

/// An opened TIFF.
pub struct File<R> {
    c: Container<R>,
    lim: Limits,
    pool: Option<ThreadPool>,
}

impl<R: ReadAt> File<R> {
    /// Reads the header and the IFD chain of the `size` bytes behind `ra`. Opening reads
    /// metadata only and uses no thread.
    ///
    /// # Errors
    /// [`Error::Limit`] when the chain has more than [`Limits::max_ifds`] directories or a
    /// directory has more than 4096 entries (`ifd entries`), [`Error::Corrupt`] for a bad header
    /// or a loop in the chain, [`Error::Io`] from `ra` (a source that ends before `size`, in the
    /// header or a directory, is `Io(UnexpectedEof)`).
    pub fn open(ra: R, size: u64, lim: &Limits) -> Result<Self, Error> {
        let c = Container::open(ra, size, lim.max_ifds)?;
        Ok(Self {
            c,
            lim: *lim,
            pool: None,
        })
    }

    /// Runs the parallel decodes of this file on `pool` instead of the crate's shared pool.
    /// [`Limits::max_threads`] still caps the workers of a call.
    pub fn with_pool(mut self, pool: ThreadPool) -> Self {
        self.pool = Some(pool);
        self
    }

    /// The number of IFDs.
    pub fn num_pages(&self) -> usize {
        self.c.offs.len()
    }

    /// Parses IFD `i`.
    ///
    /// # Errors
    /// [`Error::Corrupt`] for an index out of range or an invalid directory,
    /// [`Error::Limit`] over [`Limits::max_pixels`], [`Limits::max_alloc`] (tables read into
    /// memory) or the other caps, [`Error::Unsupported`] for valid TIFF this crate does not
    /// decode, [`Error::Io`] from the reader.
    pub fn page(&self, i: usize) -> Result<Page<'_, R>, Error> {
        if i >= self.num_pages() {
            return Err(error::corrupt(format!("page {i} out of range")));
        }
        let mut p = self.c.page(i, &self.lim)?;
        Ok(Page {
            orientations: std::mem::take(&mut p.orientations),
            xmps: std::mem::take(&mut p.xmps),
            f: self,
            p,
        })
    }
}

/// The codec state of one worker, kept across calls, pages and files: the JPEG and Deflate
/// decoders with their tables, and the buffers that a chunk would otherwise allocate. Give each
/// worker thread its own and decode every chunk of every page through it with
/// [`Page::decode_chunk_into`]. A chunk no larger than an earlier one, in compressed and in
/// decoded bytes, of a codec the `Decoder` has already used, allocates nothing
/// (`tests/reuse_alloc.rs` counts the calls to the allocator); `docs/performance.md` lists the
/// paths that still do. The memory of the largest chunk stays with the `Decoder` until it drops,
/// but every call books what the `Decoder` holds at its start in the ledger of `max_alloc` (and
/// frees what does not fit), so `max_alloc` is the peak of the call with that memory included.
/// Not booked: the fixed LZW table, read buffer and Deflate tables.
///
/// ```
/// use std::sync::atomic::AtomicBool;
/// use tiff::{Decoder, File, Limits};
///
/// # fn main() -> Result<(), tiff::Error> {
/// // A 2x2 8-bit grey TIFF, one strip.
/// let bytes: &[u8] = &[
///     0x49, 0x49, 0x2a, 0x00, 0x0c, 0x00, 0x00, 0x00, 1, 2, 3, 4, 0x08, 0x00, 0x00, 0x01, 0x03,
///     0x00, 0x01, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x01, 0x03, 0x00, 0x01, 0x00,
///     0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x01, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0x08,
///     0x00, 0x00, 0x00, 0x03, 0x01, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
///     0x06, 0x01, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x11, 0x01, 0x04,
///     0x00, 0x01, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x16, 0x01, 0x03, 0x00, 0x01, 0x00,
///     0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x17, 0x01, 0x04, 0x00, 0x01, 0x00, 0x00, 0x00, 0x04,
///     0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
/// ];
/// let limits = Limits {
///     max_threads: 1,
///     max_alloc: 64 << 20,
///     ..Limits::default()
/// };
/// let file = File::open(bytes, bytes.len() as u64, &limits)?;
/// let page = file.page(0)?;
/// let layout = page.layout();
/// assert_eq!((layout.chunk_width, layout.chunk_height, layout.chunks), (2, 2, 1));
///
/// let cancel = AtomicBool::new(false);
/// let mut dec = Decoder::new();
/// let mut chunk = Vec::new();
/// for i in 0..layout.chunks {
///     // `chunk` keeps its capacity, and `dec` its state, from one call to the next.
///     page.decode_chunk_into(i, &mut dec, &mut chunk, &cancel)?;
///     assert_eq!(chunk, [1, 2, 3, 4]);
/// }
/// // A chunk that the page does not have is refused.
/// assert!(matches!(
///     page.decode_chunk_into(1, &mut dec, &mut chunk, &cancel),
///     Err(tiff::Error::Unsupported(_))
/// ));
/// # Ok(())
/// # }
/// ```
pub struct Decoder {
    /// `None` only if a call unwound while it held the state; the next call starts afresh.
    st: Option<codec::State>,
}

impl Decoder {
    /// A `Decoder` that holds no buffers yet.
    pub fn new() -> Self {
        Self {
            st: Some(codec::State::new()),
        }
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Decoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Decoder").finish_non_exhaustive()
    }
}

/// How a page is cut into strips or tiles: see [`Page::layout`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Layout {
    /// Tiles (true) or strips.
    pub tiled: bool,
    /// Pixels in one row of a chunk: the tile width, or the image width for a strip.
    pub chunk_width: u32,
    /// Rows of a chunk: the tile length, or the rows per strip clamped to the image height. The
    /// last strip may have fewer.
    pub chunk_height: u32,
    /// Chunks in a row of chunks: tiles across the image, 1 for strips.
    pub across: u32,
    /// Rows of chunks: tiles down the image, the number of strips for strips.
    pub down: u32,
    /// `across * down`: the valid indexes of [`Page::decode_chunk_into`] are `0..chunks`, in
    /// row-major order.
    pub chunks: usize,
}

/// Describes a page's samples.
#[derive(Debug, Clone, PartialEq)]
pub struct Info {
    pub width: usize,
    pub height: usize,
    pub samples_per_pixel: usize,
    pub bits_per_sample: usize,
    /// As stored; JPEG-compressed YCbCr reports [`RGB`].
    pub photometric: usize,
    /// TIFF compression code.
    pub compression: usize,
    /// TIFF Orientation, 1 when absent.
    pub orientation: usize,
    /// Bytes in one delivered row.
    pub row_bytes: u64,
    /// `3 << bits_per_sample` entries for [`PALETTE`]: reds, greens, blues.
    pub color_map: Vec<u16>,
    pub extra_samples: Vec<u16>,
    /// 0 unless the page's last such entry is one in-range RATIONAL with a non-zero
    /// denominator.
    pub x_resolution: f64,
    pub y_resolution: f64,
    /// 2 (inch) unless the last such entry is a SHORT; its value is not range-checked.
    pub resolution_unit: usize,
    /// In file order, the tags ignored because their count or offset lies outside the file and
    /// decoding does not need them. A skipped Orientation reads as 1; a skipped XMP (700) is
    /// simply absent.
    pub skipped: Vec<u16>,
}

/// One parsed directory.
pub struct Page<'a, R> {
    /// Every Orientation (274) entry in file order, including those in [`Info::skipped`].
    pub orientations: Vec<TagEntry>,
    /// Every XMP (700) entry in file order, including those in [`Info::skipped`].
    pub xmps: Vec<TagEntry>,
    f: &'a File<R>,
    p: page::Page,
}

impl<R: ReadAt> Page<'_, R> {
    /// The page's description.
    pub fn info(&self) -> Info {
        let p = &self.p;
        let photometric = if p.photometric == page::YCBCR {
            page::RGB
        } else {
            p.photometric
        };
        Info {
            width: p.width,
            height: p.height,
            samples_per_pixel: p.spp,
            bits_per_sample: p.bps,
            photometric: photometric as usize,
            compression: p.compression as usize,
            orientation: p.orientation as usize,
            row_bytes: p.row_bytes(),
            color_map: p.color_map.clone(),
            extra_samples: p.extra_samples.clone(),
            x_resolution: p.x_res,
            y_resolution: p.y_res,
            resolution_unit: p.res_unit as usize,
            skipped: p.skipped.clone(),
        }
    }

    /// Calls `f` with consecutive bands of full-width rows starting at row `y`. A band is whole
    /// rows and at most [`Limits::max_band_bytes`]; `f` must not keep it. `cancel` is checked
    /// before every band and row, and `ControlFlow::Break` from `f` stops the decode with
    /// [`Error::Stopped`].
    ///
    /// Strips and tiles are decoded on up to [`Limits::max_threads`] threads, the calling thread
    /// included, when there are several and their buffers fit in [`Limits::max_alloc`]; `f` is
    /// always called on the calling thread, in order. The bands, the errors and the partial
    /// output are the same for every thread count, provided the reader is as [`ReadAt`]
    /// requires: its result depends only on the offset and the length, and calls may run in
    /// parallel. A reader whose failures depend on the number or the order of its calls can fail
    /// at another chunk with several threads than with one.
    ///
    /// # Errors
    /// [`Error::Limit`] when a row is over `max_band_bytes` or the call would hold more than
    /// `max_alloc` bytes, [`Error::Corrupt`] for bad data, [`Error::Unsupported`],
    /// [`Error::OutOfMemory`], [`Error::Io`] from the reader, [`Error::Cancelled`] when `cancel`
    /// is set and [`Error::Stopped`] when `f` breaks. Strip or tile data that the source cuts
    /// short is [`Error::Corrupt`].
    pub fn rows<F>(&self, cancel: &AtomicBool, mut f: F) -> Result<(), Error>
    where
        F: FnMut(usize, &[u8]) -> ControlFlow<()>,
    {
        decode::rows(
            &self.f.c,
            &self.p,
            &self.f.lim,
            self.f.pool.as_ref(),
            cancel,
            false,
            &mut f,
        )
        .0
    }

    /// How the page is cut into strips or tiles. Strips have `chunk_width` the image width,
    /// `chunk_height` the rows per strip (at most the image height) and `across` 1.
    pub fn layout(&self) -> Layout {
        let p = &self.p;
        let u32_of = |v: u64| u32::try_from(v).unwrap_or(u32::MAX);
        let chunks = usize::try_from(p.chunks()).unwrap_or(usize::MAX);
        if p.tile_w > 0 {
            Layout {
                tiled: true,
                chunk_width: u32_of(p.tile_w as u64),
                chunk_height: u32_of(p.tile_l as u64),
                across: u32_of(p.across),
                down: u32_of(p.down),
                chunks,
            }
        } else {
            Layout {
                tiled: false,
                chunk_width: u32_of(p.width as u64),
                chunk_height: u32_of(p.rows_per_strip as u64),
                across: 1,
                down: u32_of(p.chunks()),
                chunks,
            }
        }
    }

    /// Decodes strip or tile `index` (see [`Page::layout`]) into `out`, which it clears first. The
    /// samples are laid out as [`Page::rows`] delivers them (packed as stored, rows padded to a
    /// byte, 16-bit samples big-endian, predictor undone, YCbCr JPEG as RGB), `chunk_width` pixels
    /// to a row, and every row of the chunk as stored: a tile is whole, with the padding past the
    /// image edge, and the last strip has only the rows that remain. The rows of a tile past the
    /// image edge are as stored when the stream holds them, else zero: a data or reader error
    /// after the last row inside the image is not reported. `out` is empty after an error. The
    /// capacity `out` keeps is the caller's and is not booked.
    ///
    /// It runs on the calling thread only: it spawns nothing and touches no pool, and a nested
    /// jpeg-rs call uses one thread. `dec` keeps the codec state between calls (see
    /// [`Decoder`]); it may be used for any page of any file.
    ///
    /// The limits are those of the [`File`]. Its [`Limits::max_alloc`] is a ledger of this call:
    /// what `dec` kept from earlier calls (or frees, if it does not fit), the output and every
    /// buffer of the codecs are charged, a kept buffer by its capacity if that is more than the
    /// chunk needs, except the fixed LZW table, read buffer and Deflate tables (a JPEG chunk is
    /// budgeted four times [`Limits::max_band_bytes`] with a megabyte, as in `rows`, so lower it
    /// together with `max_alloc`). A chunk of more than
    /// [`Limits::max_pixels`] pixels is [`Error::Limit`] named `pixels`, one of more decoded bytes
    /// than [`Limits::max_band_bytes`] (an NDPI strip the size of the image) `band bytes`.
    ///
    /// # Errors
    /// [`Error::Unsupported`] for an `index` that is not below [`Layout::chunks`],
    /// [`Error::Limit`] over `max_pixels`, `max_band_bytes` or `max_alloc` (`alloc`),
    /// [`Error::Corrupt`] for bad data or data that the source cuts short,
    /// [`Error::OutOfMemory`], [`Error::Io`] from the reader and [`Error::Cancelled`] when
    /// `cancel` is set (checked before the chunk, before every row, and every megabyte of a
    /// JPEG chunk). The data and reader errors of a chunk are those that [`Page::rows`] gives for
    /// its rows inside the image; the limits are not: a strip over `max_band_bytes` is a `band
    /// bytes` limit here, where `rows` decodes it in bands, and `alloc` is judged for the whole
    /// chunk.
    pub fn decode_chunk_into(
        &self,
        index: usize,
        dec: &mut Decoder,
        out: &mut Vec<u8>,
        cancel: &AtomicBool,
    ) -> Result<(), Error> {
        let st = dec.st.take().unwrap_or_else(codec::State::new);
        let (r, st) = decode::chunk_into(&self.f.c, &self.p, &self.f.lim, index, st, out, cancel);
        dec.st = Some(st);
        r
    }

    /// Strip or tile `index` (see [`Page::layout`]) as a standalone JPEG stream in `out`, which
    /// it clears first, with no decode: the chunk's compressed bytes are read through the
    /// [`ReadAt`] and, when the page has JPEGTables (tag 347), merged in. The result is the SOI,
    /// the DQT and DHT segments of the tables, then the chunk after its own SOI. Decoding it
    /// with jpeg-rs gives the same pixels, and the same colour reading, as
    /// [`Page::decode_chunk_into`] gives for the chunk, when that succeeds. Other segments of the
    /// tables stream (APPn, COM, DRI, DAC) are dropped, because jpeg-rs resets what they set at the
    /// chunk's own SOI (a DAC is checked, not kept). With
    /// no JPEGTables the chunk is returned as stored. A DQT or DHT inside the chunk wins over the
    /// tables, as in the decode. `out` is empty after an error.
    ///
    /// A page with PhotometricInterpretation RGB (Aperio SVS) holds RGB samples, which the decode
    /// reads with no colour transform, while its stream may carry no marker that says so; any
    /// decoder of the stream alone would convert it as YCbCr. For such a page with 3 samples the
    /// result says RGB itself: the JFIF APP0 and Adobe APP14 segments before the first scan are
    /// dropped and an Adobe APP14 segment with transform 0 (16 bytes) follows the SOI, so
    /// libjpeg-turbo, browsers, Go and jpeg-rs with its default `ColorModel::Auto` all read RGB.
    /// Every other byte is the chunk's (or the tables'), in order.
    ///
    /// Only compression 7 with 1 or 3 samples per pixel is served, the cases that tiff-rs decodes
    /// through jpeg-rs. Four samples go through tiff-rs's own CMYK decoder, which ignores the
    /// Adobe transform, so a standalone stream would not mean the same thing: they are
    /// [`Error::Unsupported`].
    ///
    /// The frame header of the stream is read: its size and component count must be the chunk's
    /// (the tile, or the rows left in the last strip), as `decode_chunk_into` requires. The entropy
    /// data is not decoded, so a stream whose entropy data is damaged is still returned, and
    /// decoders may fill the blocks that are missing; `decode_chunk_into` reports it as
    /// [`Error::Corrupt`]. Memory: `out` grows once, by `try_reserve_exact`, to the chunk plus
    /// the merged table segments (plus the APP14 segment of an RGB page); that size is checked against [`Limits::max_alloc`] first. There
    /// is no other allocation and no thread.
    ///
    /// # Errors
    /// [`Error::Unsupported`] for an `index` that is not below [`Layout::chunks`], another
    /// compression, or a sample count other than 1 or 3, [`Error::Limit`] named `pixels` for a
    /// chunk of more than [`Limits::max_pixels`] pixels (checked before the chunk is read, as
    /// `decode_chunk_into` does) and named `alloc` over `max_alloc`, [`Error::Corrupt`] for a chunk that does not start with SOI or lies outside
    /// the file or is cut short by the source, a frame that is not the chunk's (size or component
    /// count), or tables that are not a well-formed stream of segments (no SOI, a length past the
    /// end of a DQT, DHT or DAC, a bad DAC, a frame or scan header), [`Error::OutOfMemory`],
    /// [`Error::Io`] from the reader.
    ///
    /// ```no_run
    /// use tiff::{File, Limits};
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let f = std::fs::File::open("slide.svs")?;
    /// let size = f.metadata()?.len();
    /// let file = File::open(f, size, &Limits::default())?;
    /// let page = file.page(0)?;
    /// let mut jpeg = Vec::new();
    /// for i in 0..page.layout().chunks {
    ///     // `jpeg` keeps its capacity from one call to the next.
    ///     page.raw_jpeg_into(i, &mut jpeg)?;
    ///     // `jpeg` is a complete .jpg: send it as it is.
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn raw_jpeg_into(&self, index: usize, out: &mut Vec<u8>) -> Result<(), Error> {
        rawjpeg::raw_jpeg_into(&self.f.c, &self.p, &self.f.lim, index, out)
    }

    /// [`Page::rows`] that also reports what the call used, per call and with no global state.
    ///
    /// # Errors
    /// Those of [`Page::rows`], in the first field.
    #[doc(hidden)]
    pub fn rows_probe<F>(&self, cancel: &AtomicBool, mut f: F) -> (Result<(), Error>, Probe)
    where
        F: FnMut(usize, &[u8]) -> ControlFlow<()>,
    {
        decode::rows(
            &self.f.c,
            &self.p,
            &self.f.lim,
            self.f.pool.as_ref(),
            cancel,
            true,
            &mut f,
        )
    }
}

/// What one call used, for tests: see [`Page::rows_probe`].
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    /// The most bytes the call's ledger had booked at once, an upper bound of its heap peak.
    pub charged_peak: u64,
    /// The most threads the call had running at once, the calling thread included.
    pub threads_peak: usize,
}
