# Status

Status: the port of every Go package and public API, with the Go tests ported, plus two controls
Go does not have: `Limits::max_alloc` (an enforced peak for the whole call) and
`Limits::max_threads` (strips and tiles decode in parallel; the output does not depend on it).
CI runs fmt, clippy, the tests and an aarch64 release build (`test`), the tests on native aarch64
(`arm64`) and the Go parity job (`parity`: tiff-go's oracle against `examples/oracle.rs`, with
one thread and with the default, see [parity.md](parity.md)). `tests/fuzz/` has cargo-fuzz targets
(`.github/workflows/fuzz.yml`). Speed against Go: see `bench.yml`
(`.github/workflows/bench.yml`).

- Every IFD of a file, classic and BigTIFF, either byte order.
- Strips and tiles decoded incrementally by rows: a single-strip file of any legal size streams in
  bands of at most `Limits::max_band_bytes` (default 64 MiB).
- None, PackBits, LZW, Deflate, CCITT G3/G4 and JPEG compression; predictor 2.
- Caps checked from the header before allocation: IFDs (10 000, loops refused), pixels per IFD,
  buffered bytes. Breaches return `Error::Limit`.
