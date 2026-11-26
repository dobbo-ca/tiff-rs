# Parity with Go

`tests/parity/go/upstream/` is the non-test source of dobbo-ca/tiff-go at 756f16b (with `go.mod`, `LICENSE`
and `NOTICE`; `internal/fixture`, the test fixture writer, is copied to `tests/parity/go/oracle/fixture/`
instead, since an `internal` package cannot be imported from another module). tiff-go builds with
`deflate-go` v0.4.0 and `jpeg-go` v0.8.0 and with `golang.org/x/image/ccitt`;
`tests/parity/go/deps/` holds the non-test sources of the packages tiff-go imports from them (and `ccitt`'s `reader.go` and `table.go`, with its
`LICENSE` and `PATENTS`; its `go.mod` has the requirements removed), so the Go side builds with no
network and no token. `tests/parity/go/oracle/` is a Go program with its own `go.mod` that `replace`s the four
modules with those directories. The CI job `parity` (`.github/ci/Dockerfile`, stages `go-oracle`, `go-run`,
`parity`) runs:

1. `oracle -gen tests/data /inputs` writes the inputs.
2. `oracle /inputs /go-out` decodes them with tiff-go.
3. `cargo run --release --example oracle -- -threads 1 /inputs /rust-out-1` and the same without
   `-threads` (the default `max_threads`) decode them with this crate.
4. `tests/parity/diff.sh /go-out /rust-out-1` and `tests/parity/diff.sh /go-out /rust-out` (`diff -r`) must be
   empty. A difference is a bug in the Rust port, never in the oracle. The band bytes, the band
   split and every error must not depend on `max_threads`.

## Inputs

`oracle -gen` writes (`gen.go`, `mutate.go`; its own xorshift, so the bytes do not depend on the
Go version):

- every file of tiff-go's `testdata/golden` and `testdata/synthetic` (the TIFFs as `td-<name>`; each
  `.jpg` wrapped in a one-page JPEG TIFF as `td-jpg-<name>`, and as `td-jpgany-<name>` at 16x64 as
  in `TestJPEGAnyFrameBands`), and the `FuzzTIFF` seed (`seed-<name>`);
- the files of the Go tests, written by the Go fixture writer, with real Deflate (stdlib zlib) and
  real JPEG (jpeg-go) strips, which the Rust test fixture cannot write (`tests/common` stores
  Deflate as stored blocks);
- the mutations of a subset of those (the bases): truncations (every cut of a file under 300
  bytes, else 24: `~t<cut>`), 32 bit flips (8 of them in the first 64 bytes: `~f<bit>`), 12 byte
  sets (`~s<pos>x<value>`), 60 edits of the type, count or value field of the first IFD's entries
  (`~e<offset>_<width>_<value>`), and 5 values of its next-IFD offset (`~n<k>`: 0, itself, the
  first IFD, the file length, 0xffffffff).

### Ported Go generators and tests

| Go test | Go cases | Inputs here |
|---|---|---|
| `TestRoundTrip` (`tiff_test.go`): 5 compressions x 7 sample formats x predictor x 4 layouts x byte order x BigTIFF | one subtest per combination | `rt-c<comp>-<spp>x<bps>-p<pred>-<layout>-<be\|le>-b<0\|1>`: every combination (Go's list and filter) |
| `TestMultiPage`, `TestInfoPalette`, `TestCCITT`, `TestJPEG`, `TestSingleStripStreams` | 1, 1, 2, 4, 5 | `multipage`, `multipage-big-le`, `palette`, `ccitt-g4-p<0\|1>` (Go's two photometrics), `jpeg-<strips\|tiles>-<1\|3>`, `strip-c<comp>` (600x300 instead of 2000x1000), plus CCITT group 3 variants and CCITT on RGB (refused) |
| `TestLimits`, `TestHostileHeaders`, `TestRefusals` | 6 + 2 + 7 | `lim-*` (10,000 and 10,001 IFDs, a loop, `pixels`, a huge header, a row and a tile row wider than a band), `hostile-*`, `refuse-*`; the band cases need the `s` configuration (a 2000-byte band) because the oracle's limits are not Go's test limits |
| `TestSkippedOutOfRangeTags`, `TestEntriesOverflowSafe`, `TestOrientationEntries*`, `TestXMPEntries`, `TestResolutionEntries`, `TestSkipped*`, `TestOutOfRange*`, `TestDuplicateOrientationLastWins`, `TestBadEntryThenValidDuplicateReadsValid` (`entries_test.go`, `skip_test.go`) | every combination of the 16 types, classic and BigTIFF, the counts and offsets near 2^32 and 2^63, and tags 274 and 700 | `oob-*`: the same loops, with the same `oobTIFF` writer (`oob`), and the info file lists every Orientation and XMP entry |
| `TestFillOrder2At8Bits` | 1 | `fillorder2` |
| `TestJPEGCMYKMatchesLibtiff`, `TestJPEGCMYKRestart`, `TestJPEGCMYKCorrupt`, `TestJPEGProgressiveStripBands`, `TestJPEGAnyFrameBands` | testdata files | the `td-*` inputs above, `td-cmyk-corrupt-<n>` |
| `FuzzTIFF` seeds | 10 generated + 1 corpus entry | `fz-c<comp>-<strips\|tiles>`, `seed-seed-lzw-grey` |
| `TestLibtiffOracle`, `TestOracleFiles` | the golden files | `td-<name>.tif`; they run under the default and the small band (`d` and `s`), as the Go test does |

Not ported as inputs: the Go tests that assert on memory (`TestSingleStripStreams`' allocation
bound, `TestRowsReleasesOnEarlyExit`, which uses jpeg-go's process limit) and on timing; the Rust
tests cover them (`max_alloc`, see [limits.md](limits.md)).

Counts: the `parity` job in CI writes 18,204 inputs (103 bases, 15,847 mutations) and compares 161,837 output files, all equal to Rust's; its log is authoritative, check these figures against it. The Go cases of each test are in the table above. Every combination of the Go tests' loops is an input, plus the mutations; a generated name that repeats gets a `-dup<n>` suffix, so none overwrites another and the inputs written equal the inputs compared.

## Configurations and operations

Per input the oracle runs three limits configurations, then per page (up to 64) its operations:

| config | limits | operations |
|---|---|---|
| `d` | `max_ifds` 10000, `max_pixels` 1<<22, `max_band_bytes` 1 MiB, `max_metadata_bytes` 4 MiB | `rows`, `c1`, `k1` |
| `s` | as `d` with `max_band_bytes` 2000 and `max_metadata_bytes` 8 | `rows` |
| `g` | each side's defaults: Go `tiff.Limits{}`, Rust `Limits::default()` (with `max_threads` as the run sets it) | `rows` |

Not compared: pages past 64 (`maxPages`). Under `g` Rust's `max_alloc` (1 GiB) is a limit Go has no
equivalent of (only jpeg-go's 256 MiB process limit), so an input whose decode needs more than that
can differ as `limit:alloc` against a result.

`max_alloc` is 256 MiB in Rust and jpeg-go's process memory limit is the same 256 MiB in Go, so
neither side allocates more than that for a hostile JPEG header. `rows` collects every band;
`c1` cancels (Go: the context, Rust: the flag) in the callback of the first band; `k1` returns an
error (Go) or `Break` (Rust) from it.

Each of the two Rust runs (`-threads 1` and the default) writes the same set of files per input, for configurations `d`, `s` and `g` (the defaults of each side), up to `maxPages` (64) pages per input:

Files, per input `<name>` and configuration `<cfg>`: `<name>.<cfg>.info` holds `pages=<n>` (or
`open err=<class>`), then per page either `p<i> err=<class>` or its `Info` (every field, floats as
their bits, the colour map as length and FNV-1a 64) and every Orientation and XMP entry (data as
length and FNV-1a 64). `<name>.<cfg>.p<i>.<op>.out` has the bytes of all bands, or
`<name>.<cfg>.p<i>.<op>.err` has the error class and `<name>.<cfg>.p<i>.<op>.partial.out` the bytes
delivered before the error (both sides write it, and they must be equal). `<name>.<cfg>.p<i>.<op>.bands`
has one line `y len` per band, so the row split and the start row of every band are compared too.

## Error classes

| class | Go | Rust |
|---|---|---|
| `corrupt` | `ErrCorrupt` | `Error::Corrupt` |
| `unsupported` | `ErrUnsupported` | `Error::Unsupported` |
| `limit:<name>` | `*LimitError` (and `jpeg.ErrMemoryLimit` as `limit:jpeg`) | `Error::Limit { name, .. }` |
| `cancelled` | `context.Canceled` | `Error::Cancelled` |
| `stopped` | the callback's error | `Error::Stopped` |

`<name>` is `ifds`, `ifd-entries`, `pixels`, `band-bytes` or `jpeg-tables` as tiff-go names them,
and the first word of the others (`jpeg`, `deflate`). `Error::Io`, `Error::OutOfMemory` and the
`alloc` limit have no Go counterpart; they would print as `other:<message>` or `limit:alloc` and
show up as a diff. Info's `Orientation` is a Go `int` and a Rust `usize`, and a page can store any 64-bit value in it (a LONG8 entry in a classic file): the Rust oracle prints it as `i64` so the two compare. The message of a corrupt error is not compared: Rust's carries different words.

## Benchmark

`bench.sh` and `.github/workflows/bench.yml`: `oracle -genbench tests/data /bench-in` writes 14
images of 1536x1024 (none, LZW, LZW with predictor, PackBits, Deflate, Deflate with predictor at
16 bits, JPEG, each as strips and tiles where it applies), the golden CCITT file and three large CCITT files (`ccitt-g4-large`, `ccitt-g3-1d-large`, `ccitt-g3-2d-large`: the first nine strips of `bw_g4_r16`, `bw_g3pfill_r16` and `bw_g3p2d_r16` repeated 1000 times, 203x144000, 3.7 MB of output), 400 strips of a
progressive JPEG (jpeg-go's `RowReader`) and a 4-component JPEG file (the 4-sample decoder);
`oracle -bench N` and `examples/bench.rs <dir> N [threads]` (`Limits::max_threads`) decode every page
of each N times and print the ns per decode. `bench.sh` joins the best of five runs into two tables,
each a ms/MB row per compression and a row per file: Rust at the default `max_threads` against Go at
its default, and Rust with `max_threads` 1 against Go with `GOMAXPROCS=1` (tiff-go is serial; jpeg-go
decodes restart intervals in parallel and has no knob but `GOMAXPROCS`). A ratio over 1.10 in any row
is marked and fails the script, and so does an output size that differs between Go and Rust. Both
sides run with the default limits, in one container.

The image also holds `oracle-v4` (Go built with `GOAMD64=v4`) and `bench-v4` (the bench example built with `RUSTFLAGS="-C target-cpu=x86-64-v4 -C target-feature=+false-deps-tzcnt,+false-deps-bls"` in its own target cache, `tiff-target-v4`). When both are on `PATH`, `bench.sh` runs them with the same passes and adds the columns "Go v4 ms/MB", "Rust v4 ms/MB" and "v4 Rust / Go" to each table; the v4 output sizes must equal the default ones. The v4 columns are reported, not gated: the 1.10 bound applies to the default columns only. The v4 build only fits CPUs with AVX-512; `bench.sh` skips the v4 columns on a CPU without `avx512f`. The library itself needs no flag.

`pgo.yml` (dispatch only) measures profile-guided optimisation and BOLT on the bench example. Stage `pgo` of the Dockerfile builds it with `cargo-pgo`, trains it on `/bench-in` single-threaded and with the default threads, and (when `bolt-19` installs from apt.llvm.org) runs it through BOLT. `tests/parity/pgo.sh` runs `bench.sh` once per binary (`BENCH=bench`, `bench-pgo`, `bench-bolt`) and prints, per class, the Rust ms/MB of the default, PGO and PGO plus BOLT builds with the percent change against the default build, for each pass. The library does not change: the consumer applies PGO and BOLT to its own binary.

## Real files (manual)

Not run in CI. The tools for a real-slide comparison are in the tree: `oracle -qtiles -max N -log <log> <out-dir> <slide>...` (Go, `qtiles.go`) and `examples/quality.rs` (`quality -max N -log <log> <out-dir> <slide>...`) write the same sampled 256 x 256 cells of every page as PPM/PGM, plus a log of the pages they skip or fail. The two trees and logs should be byte-identical, except on JPEG pages with PhotometricInterpretation RGB (Aperio SVS), which tiff-rs decodes as stored and tiff-go converts as YCbCr ([go-differences.md](go-differences.md)). For those pages `.github/ci/openslide-cells.py` writes OpenSlide's cells for comparison. `examples/rawjpeg.rs` checks that `Page::raw_jpeg_into` streams, decoded by jpeg-rs, equal `Page::decode_chunk_into`. Suitable inputs are the public OpenSlide test data (https://openslide.cs.cmu.edu/download/openslide-testdata/): CMU-1-Small-Region.svs, CMU-1.svs, CMU-1.ndpi, Leica-1.scn, Leica-Fluorescence-1.scn, CMU-1.tiff, Philips-4.tiff, Ventana-1.bif.
