# Differences from Go

## Input

`tiff::File::open` takes a `ReadAt` (Go's `io.ReaderAt`): positioned reads, implemented for
`[u8]`, `Vec<u8>`, `&T` and `std::fs::File` (unix, windows). As in Go, a read's result must depend
only on its offset and length (see [threading.md](threading.md)).

## Errors

One `Error` enum, one variant per Go error: `Limit { name, got, max }` is `*LimitError`
(wrapping `ErrLimit`; `name` is Go's, "ifds", "ifd entries", "pixels", "band bytes", "jpeg
tables", plus "alloc" and the "jpeg ..." / "deflate ..." names of the parts' own limits),
`Corrupt` is `ErrCorrupt`, `Unsupported` is `ErrUnsupported`, `Cancelled` is `context.Canceled`.
New: `Io(kind)` (an error from the caller's `ReadAt`, which Go also returned as it was, unwrapped;
an error from the reader is `Io(kind)` in `open`, `page` and `rows`; only a source that ends early or returns `InvalidData` inside strip or tile data is `Corrupt`), `OutOfMemory` (the allocator refused a size within the limits) and `Stopped`.

## Differences

- `ReadAt` short reads: Go returns a non-EOF error from the caller's `ReaderAt` unwrapped, and
  treats a read that ends in `io.EOF` with fewer bytes as success, leaving the rest of the buffer
  as it was (header and IFD reads). Here the error kind is returned as `Io(kind)`, and a source
  that ends before the `size` given to `File::open` is `Io(UnexpectedEof)` when the header or a
  directory is read, and `Corrupt` when strip or tile data is cut short. Only inputs opened with a `size` larger than the data differ; the oracle
  always passes the data's length, so parity does not compare them.
- `Limits` has `max_alloc` and `max_threads`, which Go has no equivalent of (see [limits.md](limits.md)).
  `ReadAt` requires `Sync`, because strips and tiles are read from several threads.
- `File::open` and `File::page` borrow instead of copying: `Page` holds `&File`.
- `Page::orientations` and `Page::xmps` are public fields; `TagEntry::data` is `Option<Vec<u8>>`
  (Go's nil versus empty).
- Go's process-wide jpeg-go memory limit (`jpeg.SetMemoryLimit`) has no jpeg-rs counterpart; the
  per-call `Limits::max_alloc` takes its place, and its tests (`TestRowsReleasesOnEarlyExit`,
  `TestJPEGLimitIsNotCorrupt`, `TestJPEGProcessLimitIsNotCorrupt`) are ported as
  `rows_release_on_early_exit`, `jpeg_limit_is_not_corrupt` and the unit test in
  `src/codec/jpeg_rows.rs`.
- Go's allocation measurement in `TestSingleStripStreams` is a `max_alloc` of 3 MiB (half the
  decoded size) here, so a decoder that held the strip would be refused.
- The fixture writes Deflate as stored blocks (no Deflate encoder in deflate-rs); the golden
  files cover real Deflate.
- Rust-only API, with no Go counterpart (additive): `Decoder` (buffers reused across chunks), `Layout` and
  `Page::layout` (how a page is cut into chunks), `Page::decode_chunk_into` (one strip or tile on
  the calling thread) and `Page::raw_jpeg_into` (a JPEG chunk as a complete .jpg).
- Deliberate difference: a JPEG page (compression 7) with 3 samples and
  PhotometricInterpretation RGB (2) is decoded as RGB as stored, with no colour transform, whatever
  the stream's JFIF, Adobe APP14 or component ids say (`jpeg::ColorModel::Rgb`). This is what
  libtiff (`JPEGPreDecode` sets `JCS_UNKNOWN` for every photometric but YCbCr) and OpenSlide
  (`decode_jpeg` sets `jpeg_color_space` from the Photometric tag, after the header) do. tiff-go, like
  Go's image/jpeg, reads the stream's markers only, so it converts such tiles as YCbCr. Aperio SVS
  stores its JPEG tiles this way (RGB, 4:4:4, ids 0, 1, 2, no JFIF or Adobe segment, JPEGTables): in
  tiff-go their colours are wrong and most tissue pixels clip. Photometric YCbCr (6) is unchanged
  (the markers decide; libtiff with `JPEGCOLORMODE_RGB` would force YCbCr even over an Adobe
  transform 0, a case not changed here). An RGB page whose JPEG components are subsampled is
  `Unsupported` here, as libtiff refuses it ("Improper JPEG sampling factors"); OpenSlide would
  upsample it and tiff-go converts it as YCbCr. `Page::raw_jpeg_into` of an RGB page inserts an
  Adobe APP14 transform 0 segment (and drops JFIF and other Adobe segments), so any decoder reads
  RGB. Parity: the Go fixture generator writes its YCbCr JPEG fixtures as Photometric 6 (they were
  YCbCr streams labelled RGB), and a real-file comparison takes the Aperio JPEG pages out of the
  Go comparison and compares them with OpenSlide instead (see [parity.md](parity.md)).
- Known Go quirk, kept for bit-exact parity: `jpeg4.go`'s `mcuRow` is never incremented, so the
  first MCU of a later band never takes a restart marker (`src/codec/jpeg4.rs`).
