# CI image: every check runs in a BuildKit build on the GitHub-hosted runner, so the toolchain is
# pinned here. The git dependencies (deflate-rs, jpeg-rs) and the fuzz crate's crates.io
# dependencies are fetched in one step; every later step runs offline. The aarch64 stages (arm64,
# fuzz on arm64) build natively on ubuntu-24.04-arm.
# Every tool is installed in a stage before `COPY . .`, so a new commit does not install it again,
# and the fetch runs from the manifests and lockfiles alone, so a commit that leaves them alone
# does not fetch again.
FROM rust:1.99.1-bookworm AS toolchain
RUN rustup component add rustfmt clippy && rustup target add aarch64-unknown-linux-gnu
WORKDIR /src

# Only the manifests, with empty targets for what they name, so cargo can resolve them.
FROM toolchain AS deps
COPY Cargo.toml Cargo.lock ./
COPY tests/fuzz/Cargo.toml tests/fuzz/Cargo.lock tests/fuzz/
RUN mkdir -p src tests/fuzz/fuzz_targets && touch src/lib.rs \
    && for t in open_decode codec_chunk entries reuse limits readat threads chunk raw_jpeg; do touch tests/fuzz/fuzz_targets/$t.rs; done
RUN cargo fetch --locked && cargo fetch --locked --manifest-path tests/fuzz/Cargo.toml \
    && rm -rf src tests/fuzz/fuzz_targets

FROM toolchain AS src
COPY --from=deps /usr/local/cargo/registry /usr/local/cargo/registry
COPY --from=deps /usr/local/cargo/git /usr/local/cargo/git
ENV CARGO_NET_OFFLINE=true
COPY . .

# The checks run in the build, so a red check fails the build; a rerun of the same source is a
# BuildKit cache hit. The target dir is a cache mount, so it persists across builds. No SIMD here,
# so there is no scalar-tier run and no NEON module to lint.
FROM src AS ci
RUN --mount=type=cache,id=tiff-target-ci,sharing=locked,target=/src/target \
    find src examples tests -type f -exec touch {} + && cargo fmt --check
RUN --mount=type=cache,id=tiff-target-ci,sharing=locked,target=/src/target \
    find src examples tests -type f -exec touch {} + && cargo clippy --locked --all-targets -- -D warnings
RUN --mount=type=cache,id=tiff-target-ci,sharing=locked,target=/src/target \
    find src examples tests -type f -exec touch {} + && cargo test --locked
RUN --mount=type=cache,id=tiff-target-ci,sharing=locked,target=/src/target \
    find src examples tests -type f -exec touch {} + && cargo build --locked --release --target aarch64-unknown-linux-gnu

# aarch64 tests, native on ubuntu-24.04-arm, only those that the target changes. The crate has no
# SIMD of its own; what is aarch64-specific is the NEON code of deflate-rs (adler32, run on every
# Deflate strip) and jpeg-rs (run on JPEG strips). The unit tests hold the JPEG row tests,
# tests/jpeg.rs decodes JPEG strips through the dependency's NEON path, and the name-filtered run
# of tests/tiff.rs (round_trip decodes Deflate, jpeg decodes JPEG) covers the Deflate path.
# entries.rs (header parsing) and the rest of tiff.rs (container logic) run the same code on both
# architectures; the full suite runs on x86_64 in `ci`.
FROM src AS arm64
RUN --mount=type=cache,id=tiff-target-arm64,sharing=locked,target=/src/target \
    find src examples tests -type f -exec touch {} + && cargo test --locked --lib --test jpeg \
    && cargo test --locked --test tiff -- round_trip jpeg

# Go parity: the Go oracle writes the inputs and decodes them; the Rust example decodes the same
# inputs; the two output trees must be equal. The Go side is vendored under tests/parity/go (tiff-go and
# the sources of deflate-go, jpeg-go and x/image/ccitt that it builds with), so it needs no network.
FROM golang:1.27-bookworm AS go-oracle
ENV GOTOOLCHAIN=local CGO_ENABLED=0
WORKDIR /src
COPY tests/parity/go tests/parity/go
RUN --mount=type=cache,id=tiff-go-build,target=/root/.cache/go-build cd tests/parity/go/oracle && go build -o /out/oracle .
# The same oracle built for x86-64-v4 (the bench's v4 column); the parity stages use /out/oracle.
RUN --mount=type=cache,id=tiff-go-build,target=/root/.cache/go-build cd tests/parity/go/oracle && GOAMD64=v4 go build -o /out/oracle-v4 .

FROM go-oracle AS go-run
COPY tests/data tests/data
RUN /out/oracle -gen tests/data /inputs && ls /inputs | wc -l && du -sh /inputs
RUN /out/oracle /inputs /go-out && ls /go-out | wc -l && du -sh /go-out

# The Go trees are bind mounts from go-run, not COPY, so a commit does not copy them again after
# `COPY . .`. The Rust oracle runs twice, with `-threads 1` and with the default max_threads, and
# both outputs must equal the Go tree. Each Rust output is deleted after its diff, so the layer
# (and the cache export) stays small.
FROM src AS parity
RUN --mount=type=cache,id=tiff-target-parity,sharing=locked,target=/src/target \
    --mount=type=bind,from=go-run,source=/inputs,target=/inputs \
    --mount=type=bind,from=go-run,source=/go-out,target=/go-out \
    find src examples tests -type f -exec touch {} + && echo "$(ls /inputs | wc -l) inputs, $(ls /go-out | wc -l) outputs" \
    && [ "$(ls /go-out | wc -l)" -ge 20000 ] \
    && cargo run --locked --release --example oracle -- -threads 1 /inputs /rust-out-1 \
    && sh tests/parity/diff.sh /go-out /rust-out-1 \
    && rm -rf /rust-out-1 \
    && cargo run --locked --release --example oracle -- /inputs /rust-out \
    && sh tests/parity/diff.sh /go-out /rust-out \
    && rm -rf /rust-out

# Fuzzing (fuzz.yml): nightly and cargo-fuzz, with the targets built into the image. The
# workflow runs it with `docker run`, so a BuildKit cache hit cannot skip the fuzzing. The same
# stage builds natively, with ASan, on x86_64 and on aarch64.
FROM toolchain AS fuzz-tools
RUN rustup toolchain install nightly --profile minimal \
    && cargo install cargo-fuzz --locked --version 0.13.1

FROM fuzz-tools AS fuzz
COPY --from=deps /usr/local/cargo/registry /usr/local/cargo/registry
COPY --from=deps /usr/local/cargo/git /usr/local/cargo/git
ENV CARGO_NET_OFFLINE=true
COPY . .
RUN cargo +nightly fuzz build --fuzz-dir tests/fuzz

# Benchmark (bench.yml): the Go oracle, the Rust bench example and the generated inputs in one image,
# each built twice: the default build, and x86-64-v4 (GOAMD64=v4; RUSTFLAGS as in docs/performance.md)
# as oracle-v4 and bench-v4; tests/parity/bench.sh prints both.
FROM go-oracle AS go-bench
COPY tests/data tests/data
RUN /out/oracle -genbench tests/data /bench-in && du -sh /bench-in

FROM src AS bench
COPY --from=go-oracle /out/oracle /usr/local/bin/oracle
COPY --from=go-oracle /out/oracle-v4 /usr/local/bin/oracle-v4
COPY --from=go-bench /bench-in /bench-in
# The target cache is shared by every branch and cargo compares mtimes: another branch's newer build
# would be reused as this one's, so the sources are touched before each build.
RUN --mount=type=cache,id=tiff-target-bench,sharing=locked,target=/src/target \
    find src examples tests -type f -exec touch {} + \
    && cargo build --locked --release --example bench && cp target/release/examples/bench /usr/local/bin/bench
# Its own cache mount, since RUSTFLAGS changes every artifact. `--target` keeps RUSTFLAGS off the
# build scripts, which run here on a runner that may have no AVX-512.
RUN --mount=type=cache,id=tiff-target-v4,sharing=locked,target=/src/target \
    find src examples tests -type f -exec touch {} + \
    && RUSTFLAGS="-C target-cpu=x86-64-v4 -C target-feature=+false-deps-tzcnt,+false-deps-bls" cargo build --locked --release --target x86_64-unknown-linux-gnu --example bench \
    && cp target/x86_64-unknown-linux-gnu/release/examples/bench /usr/local/bin/bench-v4

# PGO and BOLT (pgo.yml, dispatch only): cargo-pgo builds the bench example instrumented, trains it
# on the benchmark inputs, rebuilds it with the profile, and, when BOLT is installed, once more
# through BOLT. The tools are installed before `COPY . .`, so a new commit does not install them
# again. BOLT comes from apt.llvm.org (bookworm has none of its own); if the repository or the
# package is missing the image still builds and the job reports PGO only.
FROM toolchain AS pgo-tools
RUN (rustup component add llvm-tools || rustup component add llvm-tools-preview) \
    && cargo install cargo-pgo --locked --version 0.3.0
RUN apt-get update && apt-get install -y --no-install-recommends wget gnupg \
    && { wget -qO- https://apt.llvm.org/llvm-snapshot.gpg.key | gpg --dearmor -o /usr/share/keyrings/llvm.gpg \
         && echo "deb [signed-by=/usr/share/keyrings/llvm.gpg] http://apt.llvm.org/bookworm/ llvm-toolchain-bookworm-19 main" > /etc/apt/sources.list.d/llvm.list \
         && apt-get update && apt-get install -y --no-install-recommends bolt-19 libbolt-19-dev \
         && ln -s /usr/lib/llvm-19/bin/llvm-bolt /usr/local/bin/llvm-bolt \
         && ln -s /usr/lib/llvm-19/bin/merge-fdata /usr/local/bin/merge-fdata \
         && ln -s "$(find /usr/lib/llvm-19 -name libbolt_rt_instr.a | head -n 1)" /usr/local/lib/libbolt_rt_instr.a \
         && test -s /usr/local/lib/libbolt_rt_instr.a; } \
    || echo "BOLT is not available: the PGO job reports PGO only"; \
    rm -rf /var/lib/apt/lists/*

# bench (default build), bench-pgo and, when BOLT worked, bench-bolt (PGO then BOLT), all in
# /usr/local/bin with the Go oracle and the inputs; tests/parity/pgo.sh times them. The training
# runs the bench example single-threaded and with the default threads. The profiles of an earlier
# run stay in the cache mount, so they are deleted first.
FROM pgo-tools AS pgo
COPY --from=deps /usr/local/cargo/registry /usr/local/cargo/registry
COPY --from=deps /usr/local/cargo/git /usr/local/cargo/git
ENV CARGO_NET_OFFLINE=true
COPY . .
COPY --from=go-oracle /out/oracle /usr/local/bin/oracle
COPY --from=go-bench /bench-in /bench-in
RUN --mount=type=cache,id=tiff-target-pgo,sharing=locked,target=/src/target \
    find src examples tests -type f -exec touch {} + \
    && rm -rf target/pgo-profiles target/bolt-profiles && find target -name 'bench-bolt-*' -delete \
    && host=$(rustc -vV | sed -n 's/^host: //p') && bin=target/$host/release/examples/bench \
    && cargo build --locked --release --example bench && cp target/release/examples/bench /usr/local/bin/bench \
    && cargo pgo build -- --locked --example bench \
    && cargo pgo run -- --locked --example bench -- /bench-in 5 1 \
    && cargo pgo run -- --locked --example bench -- /bench-in 5 \
    && cargo pgo optimize build -- --locked --example bench && cp $bin /usr/local/bin/bench-pgo \
    && if command -v llvm-bolt >/dev/null && command -v merge-fdata >/dev/null; then \
         { cargo pgo bolt build --with-pgo -- --locked --example bench \
           && "$(dirname $bin)/bench-bolt-instrumented" /bench-in 5 1 >/dev/null \
           && cargo pgo bolt optimize --with-pgo -- --locked --example bench \
           && cp "$(find target -type f -name 'bench-bolt-optimized' | head -n 1)" /usr/local/bin/bench-bolt; } \
         || echo "BOLT failed: the job reports PGO only"; \
       else echo "no BOLT: the job reports PGO only"; fi
