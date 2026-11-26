# Threading

`max_threads = 1` runs on the calling thread: no thread is spawned and no pool is touched, not
even to create it.

With more, `Page::rows` decodes in parallel:

- strips, when a strip fits one band (`rows_per_strip` rows within `max_band_bytes`); a strip
  split into several bands is decoded serially;
- tiles, every tile of the page, delivered one tile row at a time.

A call uses at most `max_threads` threads (calling thread included), no more than it has strips
or tiles, no more than fit in `max_alloc`, and no more than the pool has plus one. The callback
runs on the calling thread, in order. For a `ReadAt` whose result depends only on the offset and the length, the bands, their start rows, the bytes, the errors and the
partial output before an error are identical for every value of `max_threads` (the tests and the
parity job check it for 1 and the default). A worker whose chunk fails does not report: the
caller stops the workers and decodes from that chunk serially, so every error is the serial one.

The workers come from a pool. A `tiff::ThreadPool` given with `File::with_pool` always wins. Else
a lazy shared pool (`OnceLock`) of `available_parallelism` threads, with no other state, is
created by the first call that has `max_threads` above 1 and at least two chunks to decode. Nested
`jpeg-rs` calls get `max_threads` 1 when tiff runs more than one worker (the workers are the
parallelism), and the call's `max_threads` otherwise.

A `tiff::ThreadPool` owns two sets of threads of the same count, its own and a `jpeg::ThreadPool`.
A call that runs serially (one strip or tile, or a strip split in bands) with `max_threads` above
1 hands the caller's pool to `jpeg-rs`, whose parallel passes (the band decode of a scan with restart markers, from about 16 KiB of entropy data, and the colour conversion of a frame of about 262 144 pixels of decoded region) then run on it. Without a pool, such a call runs `jpeg-rs` on its own lazy shared pool, a
second process-wide pool of `available_parallelism` threads, started by the first JPEG large
enough to thread (see the thresholds above). `max_threads` caps the threads of the call either way.

The identical-output guarantee assumes a `ReadAt` as Go's `io.ReaderAt` is: its result depends
only on the offset and the length, and calls may run in parallel. A source whose failures depend
on the number or the order of the calls can fail at another chunk with several threads, and
deliver fewer bands, than with one.
