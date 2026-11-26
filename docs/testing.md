# Tests

`cargo test` runs: `tests/tiff.rs` (`tiff_test.go`, `hostile_test.go`, `libtiff_test.go`,
`example_test.go`, and `FuzzTIFF`'s seeds plus deterministic mutations of them under tight
limits), `tests/entries.rs` (`entries_test.go`, `skip_test.go`), `tests/jpeg.rs`
(`jpeg4_test.go`, `jpeg_progressive_test.go`), `tests/resources.rs` (for `max_threads` 1, 2, 4 and
the default and several `max_alloc` and band sizes: the bands, errors and partial output equal the
serial ones, the ledger's peak stays within `max_alloc`, the thread peak within `max_threads`;
`Break` and cancel behave as serial), `tests/pool.rs` (`max_threads` 1 never starts the shared
pool, a caller's `ThreadPool` is used instead of it, a call worth threading starts it),
`tests/nested.rs` (`rows` from inside the callback on a shared pool finishes), `tests/jpeg_pool.rs`
(on a large JPEG strip, `decode_chunk_into` at `max_threads` 4 spawns no thread and starts no pool; with a caller's pool `rows` does not start either shared pool, and the thread peak, best of 30 tries, is above 1 at `max_threads` 2 and 4 with and without a pool, so the nested `jpeg-rs` threads are counted),
`tests/chunk.rs` (`Page::decode_chunk_into` for every chunk of every fixture page, assembled, equals `rows`, with one `Decoder`; the errors and limits of one chunk, among them `pixels`; on one-strip pages with one byte changed, the chunk call and `rows` end the same way; a bottom tile whose stream holds only the visible rows is Ok with zero padding), `tests/raw_jpeg.rs` (`Page::raw_jpeg_into`: the stream decoded with jpeg-rs equals `decode_chunk_into` for every JPEG fixture chunk, with and without tables, with Adobe, DRI, JFIF and COM segments in the tables, with fill bytes, TEM, RSTn, short APPn and DNL segments and junk after the tables' EOI, with a cut trailing COM, with a DAC; a frame that is not the chunk's is Corrupt in both calls; its errors), `tests/reuse_alloc.rs` (a counting global allocator: the second decode of a chunk on a `Decoder` makes no allocator call, for each codec class, and a smaller Deflate chunk after a larger one makes none; jpeg-rs promises none only for a frame of the same shape, so a smaller JPEG frame is not measured), `tests/heap.rs` (a counting global allocator: the live-heap peak of `rows` stays within
`max_alloc` plus a fixed allowance, and that of `rows_probe` within the bytes the call booked plus the allowance, on small fixtures, a large JPEG strip, the same JPEG in strips and a tiled JPEG page whose chunks differ in size at 2 and 4 threads; the same bound for `decode_chunk_into` over every fixture with a cold, a warm and a larger-warmed `Decoder`, and for `raw_jpeg_into`) and unit
tests (`internal/ifd/readat_test.go`, `internal/codec/jpeg_test.go`, the `ReadAt` impls, the
ledger). The synthetic fixture writer (`tests/common`) ports `internal/fixture`. `tests/data/` is
Go's `testdata/`, copied unchanged. The doc-test in `src/lib.rs` is the resource-limits example.
