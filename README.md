# tiff-rs

Multi-page TIFF and BigTIFF decoding under strict resource limits, in Rust. A port of
[dobbo-ca/tiff-go](https://github.com/dobbo-ca/tiff-go) at 756f16b (design: its `docs/design.md`).
Pure Rust, no `unsafe`, no asm. MIT licensed; see `NOTICE`. It depends on the
[deflate-rs](https://github.com/dobbo-ca/deflate-rs) and [jpeg-rs](https://github.com/dobbo-ca/jpeg-rs)
crates ([docs/dependencies.md](docs/dependencies.md)).

Dependency: `tiff = { git = "https://github.com/dobbo-ca/tiff-rs", rev = "<sha>" }`

## Quick start

```rust
use std::ops::ControlFlow;
use std::sync::atomic::AtomicBool;
use tiff::Limits;

let file = std::fs::File::open("slide.tif")?;
let size = file.metadata()?.len();
// At most 2 threads and 256 MiB for any one call.
let limits = Limits {
    max_threads: 2,
    max_alloc: 256 << 20,
    ..Limits::default()
};
let f = tiff::File::open(file, size, &limits)?;
let cancel = AtomicBool::new(false);
for i in 0..f.num_pages() {
    f.page(i)?.rows(&cancel, |y, band| {
        /* whole rows, top to bottom */
        ControlFlow::Continue(())
    })?;
}
```

## Resource limits

`File::open` keeps the limits for every call. There is no unlimited entry point; a zero field is a real cap. `Page::rows` takes a cancel flag (`Error::Cancelled`).

| Field | Default | Unit | Over it |
|---|---|---|---|
| `max_ifds` | 10 000 | directories in the chain | `Error::Limit` `ifds` (from `File::open`; a loop in the chain is `Error::Corrupt`) |
| `max_pixels` | 178 956 970 | width x height of one IFD | `Error::Limit` `pixels` (`File::page`; also a chunk of `decode_chunk_into` and `raw_jpeg_into`) |
| `max_band_bytes` | 64 MiB | decoded bytes buffered at once | `Error::Limit` `band bytes`, when one row or one tile row is larger |
| `max_metadata_bytes` | 4 MiB | XMP bytes read per page | no error: an XMP value over it is not read and its `TagEntry` says so |
| `max_alloc` | 1 GiB | bytes, peak of one call | `Error::Limit` `alloc` (also `decode_chunk_into`, which counts a `Decoder`'s buffers) |
| `max_threads` | `std::thread::available_parallelism()` | threads, calling thread included | none; 0 counts as 1 |

- `max_alloc` is the enforced peak of one call, all threads together, nested `deflate-rs` and
  `jpeg-rs` calls included. `max_threads = 1` spawns no thread; a `File::with_pool` pool wins.

Details: [docs/limits.md](docs/limits.md), [docs/threading.md](docs/threading.md).

## Docs

- [docs/status.md](docs/status.md): what is ported, CI, supported features
- [docs/go-differences.md](docs/go-differences.md): input, errors, differences from Go
- [docs/performance.md](docs/performance.md): build profile, reusing a `Decoder` per worker, PGO
- [docs/testing.md](docs/testing.md): tests
- [docs/parity.md](docs/parity.md): Go parity job
- Fuzz targets: `tests/fuzz/` (`.github/workflows/fuzz.yml`)
