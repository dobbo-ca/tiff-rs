# Performance

How to build and call this crate for speed. The timing tables are in the bench output
(`.github/workflows/bench.yml`, `tests/parity/bench.sh`).

## Build profile

A dependent crate's own release profile applies to this code, so the profile that counts is the
consumer's. For a worker that decodes tiles:

```toml
# The binary's Cargo.toml (profiles of dependencies are ignored).
[profile.release]
lto = "fat"
codegen-units = 1
panic = "abort"
```

```sh
RUSTFLAGS="-C target-cpu=x86-64-v4 -C target-feature=+false-deps-tzcnt,+false-deps-bls" cargo build --release
```

- `lto = "fat"` and `codegen-units = 1` let the decode loops and the kernels of `deflate` and
  `jpeg` inline across modules and across the crate boundary.
- `panic = "abort"` removes the unwinding tables. This crate does not panic on any input (clippy
  denies `unwrap`, `expect` and `panic` in its non-test code, and the fuzz targets run on every
  codec); a job that panics on a pool thread is re-raised on the calling thread, and with `abort`
  the process stops at the panic instead, which only matters for a bug.
- `-C target-cpu=x86-64-v4` lets the compiler use AVX-512 everywhere. **Use it only when every
  machine that runs the binary has AVX-512.** On a CPU without it the program dies with `SIGILL`
  (illegal instruction) at the first such instruction, with no message. The default build needs no
  flag and runs everywhere, because `deflate` and `jpeg` choose their SIMD tier at run time. Do not
  set the flag for an aarch64 build.
- `-C target-feature=+false-deps-tzcnt,+false-deps-bls` belongs to the v4 recommendation. Zen 4 and
  Zen 5 make `tzcnt`, `blsr`, `blsi` and `blsmsk` wait for the last write of their output register,
  and the `x86-64-v4` tuning does not break that wait, which slows the Huffman decoders of the two
  dependencies (see their `docs/performance.md`). rustc warns that it does not know the two features
  and passes them to LLVM, which does.

## Reusing a `Decoder`

A worker that decodes chunk after chunk keeps one `tiff::Decoder` and one output `Vec`, and decodes
every strip or tile with `Page::decode_chunk_into`:

```rust
let limits = tiff::Limits { max_threads: 1, ..tiff::Limits::default() };
let cancel = AtomicBool::new(false);
let mut dec = tiff::Decoder::new();
let mut chunk = Vec::new();
for path in files {
    let file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let f = tiff::File::open(file, size, &limits)?;
    let page = f.page(0)?;
    let layout = page.layout();
    for i in 0..layout.chunks {
        page.decode_chunk_into(i, &mut dec, &mut chunk, &cancel)?;
        // `chunk`: layout.chunk_height rows of layout.chunk_width pixels, as `rows` lays them out
    }
}
```

The `Decoder` holds the `jpeg::Decoder` (its planes), the `deflate::zlib::Decoder` (its Huffman
tables), the LZW code table, the read buffer, the compressed and decoded bytes of a Deflate chunk,
the compressed bytes and the band copy of a JPEG chunk, and the two rows of a CCITT chunk. After
the first chunk of a codec, a chunk no larger in both compressed and decoded bytes makes no call to the allocator
(`tests/reuse_alloc.rs` counts them for none, PackBits, LZW, Deflate with and without a predictor,
baseline JPEG grey and RGB, strips and tiles, and CCITT Group 4, and asserts 0). One `Decoder` can be
used for any page of any file and any codec; it keeps the buffers of the largest chunk it has seen.
Every call books them: at its start in its own ledger (what does not fit is freed), then each
buffer used by its capacity if that is more than the chunk needs, so `max_alloc` is the true peak
of the call, retained memory included. The data and reader errors for the rows inside the image
are those of `rows`; the limits are not always (a strip over `max_band_bytes` is a `band bytes`
limit here, where `rows` decodes it in bands). The LZW table (about 16 KiB), the 4 KiB read buffer
and the Deflate decoder's tables are fixed in size and stay unbooked.

- Give each worker thread its own `Decoder`; it is `Send` (it takes `&mut self`).
- `decode_chunk_into` never spawns a thread and never touches a pool, whatever `max_threads` is;
  a nested jpeg-rs call has one thread. Run chunks on your own threads. `Page::rows` itself keeps
  its pool for the many-chunk case and decodes Deflate chunks through the same reusable decoder
  (one state per pool task: a task that returns when the window is full starts cold, and the
  next task builds its own).
- Deflate chunks are decoded whole with `deflate::zlib::Decoder::decode_limit_into`, never with
  `decode_parallel_into` (slower on slide data, deflate-rs `docs/threading.md`), when the decoded
  chunk fits `max_band_bytes` and the ledger has room for the compressed bytes, the decoded bytes
  and the reader's reservation. A chunk that does not, or that fails to decode (bad data, a short
  chunk, a bad checksum, a failing source, a cancel), is decoded again by the streaming
  `zlib::Reader`, so every error and every partial output is the one the reader gives.

Paths that still allocate on every call, and are not in `tests/reuse_alloc.rs`:

- A progressive or non-interleaved JPEG frame: `jpeg::RowReader` builds its coefficient and band
  buffers on every call (jpeg-rs puts `RowReader` out of its reuse scope).
- A four-component (CMYK) JPEG chunk, which `src/codec/jpeg4.rs` decodes with its own buffers.
- A Deflate chunk that is decoded by the streaming reader (see above): the reader builds its window.
- The first chunk of each codec, which sizes the buffers and builds the tables, and CCITT's code
  tables, built once per process.
- Small fixed tables are not booked in the ledger: the LZW table (about 16 KiB), the 4 KiB read
  buffer and the Deflate decoder's tables, as before.

## JPEG pass-through

A tile server that sends JPEG to the browser need not decode and re-encode a JPEG tile.
`Page::raw_jpeg_into(index, &mut out)` reads the chunk's compressed bytes through the `ReadAt`
and returns them as a standalone JPEG stream, with no decode; `index` is the one of
`Page::layout` and `Page::decode_chunk_into`.

```rust
let mut jpeg = Vec::new();
page.raw_jpeg_into(index, &mut jpeg)?; // a complete .jpg: send it as it is
```

- **Tables.** Most TIFF JPEG tiles carry no tables of their own: the page has them once, in
  JPEGTables (tag 347). The call writes SOI, the tables' DQT and DHT segments, then the tile
  after its SOI. With no JPEGTables the tile is returned as stored (it must start with SOI).
- **Colour.** jpeg-rs forgets what the APPn, COM and DRI segments of a tables stream set when it
  reads the tile's own SOI, so the call drops them and keeps only DQT and DHT (a DAC is checked
  as jpeg-rs checks it, not copied). The stream
  then means to a decoder what the tile and its tables mean to tiff-rs: the JFIF and Adobe
  APP14 markers, the colour transform and the restart interval are those of the tile alone.
  The pixels are the ones `decode_chunk_into` gives, YCbCr converted as jpeg-rs converts it
  (`tests/raw_jpeg.rs` checks this for every JPEG fixture; `examples/rawjpeg.rs` does the same for
  every JPEG tile of a real file, see docs/parity.md). The call reads
  the frame header and returns `Error::Corrupt` unless the frame is the chunk's (its width, its
  height, its component count). The stream is the whole tile, including the padding past the
  image edge of an edge tile: crop in the client if the image edge matters. A stream whose
  entropy data is damaged is still returned, with no decode, and decoders may fill the missing
  blocks; `decode_chunk_into` reports it as `Error::Corrupt`.
- **Not supported.** Compression other than 7, and any sample count but 1 or 3, return
  `Error::Unsupported`. Four samples (CMYK, YCCK) are decoded by tiff-rs's own decoder, which
  ignores the Adobe transform; a standalone stream would be read by other software with the
  transform applied, so it would not mean what tiff-rs delivers.
- **Cost.** One read of the chunk and one copy; `out` grows once, by `try_reserve_exact`, to the
  chunk plus the table segments, after the size is checked against `Limits::max_alloc`. The
  stream is not parsed past the tables, so a damaged tile is passed on and fails in the
  decoder. `cargo run --release --example bench -- --rawtile <file> <n>` prints the time per
  tile against a decode (the bench job prints it for `jpeg-rgb-tile.tif`; not a gate).

## Hosted bench baseline

`bench.yml` run 45729756221 on a GitHub-hosted `ubuntu-24.04` runner (2 vCPU, no AVX-512, so no
v4 columns), the baseline for later hosted runs. Rust ms/MB over Go's per codec; shared VMs are
noisy ([parity.md](parity.md) has the method).

| input | files | MB out | default threads: Rust / Go | 1 thread: Rust / Go |
|---|---|---|---|---|
| none | 3 | 12.58 | 0.96 | 0.90 |
| lzw | 3 | 14.16 | 0.53 | 0.52 |
| packbits | 2 | 9.44 | 0.87 | 0.72 |
| deflate | 3 | 12.58 | 0.38 | 0.59 |
| jpeg | 5 | 14.48 | 0.76 | 0.75 |
| ccitt | 2 | 3.75 | 0.89 | 0.96 |
| all | 18 | 66.99 | 0.80 | 0.85 |

One file is over 1.10 at default threads: `jpeg4-cmyk-strip.tif` (0.01 MB out) at 1.13; the same
file is 0.84 single-threaded, so this is noise on a very small input.

## PGO and BOLT

Profile-guided optimisation and BOLT apply to the final binary, so the consumer does them (the
worker that links this crate); the library does not change.

Measured with `.github/workflows/pgo.yml` (dispatch only; `tests/parity/pgo.sh`, `cargo pgo` 0.3.0,
BOLT 19): the bench example is built three ways and timed on the bench inputs, after training on
the same inputs, so the gain is an upper bound. Change in Rust ms per MB of output against the
default build (`codegen-units = 1`, no LTO), measured on the old dedicated machine before
publication. The hosted baseline (runner type `ubuntu-24.04`) replaces these numbers after the
first hosted bench run:

| class | 1 thread: PGO | 1 thread: PGO + BOLT | default threads: PGO | default threads: PGO + BOLT |
|---|---|---|---|---|
| none | +0.0% | -1.4% | +3.8% | +0.0% |
| lzw | -2.4% | -3.1% | +0.2% | -3.0% |
| packbits | -18.9% | -17.6% | -10.0% | -10.0% |
| deflate | -0.8% | -1.7% | -1.4% | +2.7% |
| jpeg | -7.2% | -8.0% | -6.8% | -5.6% |
| ccitt | -25.4% | -17.1% | -22.3% | -18.7% |
| all | -20.3% | -14.2% | -17.8% | -15.1% |

BOLT on top of PGO gains nothing here (CCITT gets slower again). An earlier run, before the
Decoder reuse, showed LZW 11-14% slower with PGO; it no longer is. A consumer must train on its
own tile mix.
