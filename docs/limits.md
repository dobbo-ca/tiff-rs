# Resource limits

`Limits` bounds the memory and the threads of every call. To run in a small pod, one thread and
64 MiB:

```rust
let limits = Limits {
    max_threads: 1,
    max_alloc: 64 << 20,
    ..Limits::default()
};
let f = tiff::File::open(file, size, &limits)?;
```

`File::open` keeps the limits; every `page` and `rows` call uses them. There is no unlimited entry
point, and a zero field is a real cap (Go took zero for "use the default").

| Field | Default | Unit | Over it |
|---|---|---|---|
| `max_ifds` | 10 000 | directories in the chain | `Error::Limit` `ifds` (from `File::open`; a loop in the chain is `Error::Corrupt`) |
| `max_pixels` | 178 956 970 | width x height of one IFD | `Error::Limit` `pixels` (from `File::page`) |
| `max_band_bytes` | 64 MiB | decoded bytes buffered at once | `Error::Limit` `band bytes`, when one row or one tile row is larger |
| `max_metadata_bytes` | 4 MiB | XMP bytes read per page | no error: an XMP value over it is not read and its `TagEntry` says so |
| `max_alloc` | 1 GiB | bytes, peak of one call | `Error::Limit` `alloc` |
| `max_threads` | `std::thread::available_parallelism()` | threads, calling thread included | none; 0 counts as 1 |

Two fixed caps are not fields: more than 4096 entries in one directory is `Error::Limit`
`ifd entries` (from `File::open`), and a JPEGTables value over 1 MiB is `Error::Limit` `jpeg tables`. A failure of
`deflate-rs` or `jpeg-rs` over its own limit is `Error::Limit` named for that part (`deflate ...`,
`jpeg ...`).

## `max_alloc`

`max_alloc` is the enforced peak working memory of one `page` or `rows` call, all threads
together, including the nested `deflate-rs` and `jpeg-rs` calls. It is a ledger, not an estimate:
every buffer that scales with the input (the band and tile buffers, a JPEG chunk and the planes
its decoder keeps, the Deflate window and read step) is charged before it is allocated and
released when it drops (`src/ledger.rs`). A call that has less room than its threads would want
runs on fewer threads, with the same output; it fails only when the serial call would.

Not charged:

- Page-parse metadata: XMP values and the JPEGTables value are charged at `File::page`, against
  `max_alloc`, but the `Page` then holds them (`Page::xmps`, the tables) outside the ledger of its
  `rows` calls, and they stay until the `Page` drops.
- Fixed per-directory structures: the IFD offsets (at most `max_ifds` entries), one directory's
  entry buffer and tag map (at most 4096 entries), the colour map (at most 768 values) and the
  extra-samples list.
- The codecs' fixed tables (about 16 KiB for LZW, the CCITT code tables, the JPEG tables).
- The stacks of pool threads (512 KiB each), which belong to the pool, not to a call.
- Rayon's job bookkeeping for the pool tasks of a call (small, fixed per task).

## Threads

See [threading.md](threading.md).

## Cancellation

`Page::rows` takes an `&AtomicBool`; set it from any thread. It is checked before every band and
tile, in the CCITT and JPEG decoders (which also pass it to jpeg-rs), and between 1 MiB blocks
read from a JPEG chunk, and the call returns `Error::Cancelled`. The callback returns `ControlFlow`; `Break` ends the call with
`Error::Stopped` (Go returns the callback's own error). `File::open` and `File::page` take no
flag, as in Go; they read metadata only and use no thread.

## One chunk: `decode_chunk_into`

`Page::decode_chunk_into` decodes one strip or tile on the calling thread (see
[performance.md](performance.md)). It uses the limits of the `File`: `max_alloc` is a ledger of
that one call. At its start the call books what the `Decoder` kept from earlier calls (the JPEG
decoder under its reservation, the buffers by their capacity) and frees what does not fit; then
it charges the output and every codec buffer, a kept buffer by its capacity if that is more than
the chunk needs (freed first if that does not fit). So `max_alloc` is the peak of the call with
the retained memory included. Not booked: the fixed LZW table (about 16 KiB), the 4 KiB read
buffer and the Deflate decoder's tables, and the capacity of the caller's `out`. A chunk over
`max_pixels` is `Error::Limit` `pixels` (from `File::page` for a whole page, from
`decode_chunk_into` and `raw_jpeg_into` for a chunk), and one whose decoded size is over `max_band_bytes` (an NDPI
strip the size of the image) is `band bytes`; `rows` decodes such a strip in bands, so the limits
differ there. The data and reader errors for the rows inside the image are those of `rows`; a tile
whose stream ends after its last visible row is `Ok`, with the padding rows past the image edge
as stored when the stream holds them, else zero. `Page::raw_jpeg_into` takes `max_pixels` of the chunk (before it reads) and `max_alloc` for its
output. `max_threads` has no effect: it never spawns.
