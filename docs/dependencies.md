# Dependencies

Two git dependencies pinned by rev, and one crates.io dependency:

| Crate | Used for |
|---|---|
| `deflate` (dobbo-ca/deflate-rs, rev `211232f`) | the streaming zlib reader for Deflate strips and tiles, and the reusable `zlib::Decoder` for chunks decoded whole |
| `jpeg` (dobbo-ca/jpeg-rs, rev `c0cf0fc`) | JPEG strips and tiles: region decode per band through a reusable `Decoder`, `RowReader` for progressive frames; `ColorModel::Rgb` for Photometric RGB pages |
| `rayon` (`=1.12.0`, exact) | the worker pool (`tiff::ThreadPool`, the shared pool): safe Rust cannot reuse a thread for jobs that borrow short-lived buffers, and rayon's scope can. Used only in `src/par.rs`, as in jpeg-rs |

Both git dependencies are public; cargo fetches them with no credentials. CCITT is ported from
`golang.org/x/image/ccitt` into this crate (`src/codec/ccitt.rs`; licence in `NOTICE`).

Not a dependency of the crate: `.github/ci/openslide-cells.py` (OpenSlide via Debian
`python3-openslide`, with Pillow) writes reference cells for a manual comparison against
`examples/quality.rs` on the Aperio slides (see docs/parity.md).
