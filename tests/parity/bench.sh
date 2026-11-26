#!/bin/sh
# Decode time of the benchmark inputs, Go and Rust on the same files, as two markdown tables:
# tests/parity/bench.sh [N]. Needs `oracle` (Go), `bench` (Rust) and the inputs in /bench-in (made by
# `oracle -genbench`); run from the repo root. The Rust binary is $BENCH (default bench; pgo.sh sets
# it). When `oracle-v4` (Go built with GOAMD64=v4) and $BENCH_V4 (default bench-v4, Rust built with
# -C target-cpu=x86-64-v4 and the false-dependency flags of the Dockerfile) are on PATH too
# and /proc/cpuinfo lists avx512f, the same passes run with them and each table gets three more
# columns; the output bytes of a v4 build must equal the default build's. The v4 columns are
# reported, not gated.
#
#   1. Rust at its default max_threads against Go at its default (tiff-go is serial; jpeg-go
#      decodes restart intervals in parallel at the default GOMAXPROCS).
#   2. Rust with max_threads 1 against Go with GOMAXPROCS=1 (jpeg-go has no other knob).
#
# Each table has a row per compression and a row per file. The script exits 1 when a
# compression's ratio is over 1.10 (the bound is per class) or an output size differs between Go
# and Rust. A file over 1.10 is marked but does not fail the run: jpeg-grey-strip is 1.2x Go at
# one thread inside jpeg-rs itself (the raw1 lines below show the raw decode ratio).
set -eu
export LC_ALL=C
n=${1:-20}
grep -m1 'model name' /proc/cpuinfo >&2 || true
d=/bench-in
bench=${BENCH:-bench}
bench4=${BENCH_V4:-bench-v4}
v4=0
if command -v oracle-v4 >/dev/null 2>&1 && command -v "$bench4" >/dev/null 2>&1 && grep -q avx512f /proc/cpuinfo; then v4=1; fi
tab=$(printf '\t')
# Best of five runs per file and side: the machine is shared, and one run can be slow.
best() {
  awk -F'\t' '!($1 in m) || $3 < m[$1] { m[$1] = $3; b[$1] = $2 } END { for (k in m) printf "%s\t%s\t%s\n", k, b[k], m[k] }' "$@" | sort
}
for i in 1 2 3 4 5; do
  oracle -bench "$n" $d >/tmp/go-def-$i.tsv
  GOMAXPROCS=1 oracle -bench "$n" $d >/tmp/go-one-$i.tsv
  $bench $d "$n" >/tmp/rs-def-$i.tsv
  $bench $d "$n" 1 >/tmp/rs-one-$i.tsv
  if [ "$v4" = 1 ]; then
    oracle-v4 -bench "$n" $d >/tmp/gov4-def-$i.tsv
    GOMAXPROCS=1 oracle-v4 -bench "$n" $d >/tmp/gov4-one-$i.tsv
    $bench4 $d "$n" >/tmp/rsv4-def-$i.tsv
    $bench4 $d "$n" 1 >/tmp/rsv4-one-$i.tsv
  fi
done
best /tmp/go-def-?.tsv >/tmp/go-def.tsv
best /tmp/go-one-?.tsv >/tmp/go-one.tsv
best /tmp/rs-def-?.tsv >/tmp/rs-def.tsv
best /tmp/rs-one-?.tsv >/tmp/rs-one.tsv
if [ "$v4" = 1 ]; then
  for v in gov4-def gov4-one rsv4-def rsv4-one; do best /tmp/$v-?.tsv >/tmp/$v.tsv; done
fi

# table TITLE GO.tsv RUST.tsv [GOV4.tsv RUSTV4.tsv]: name, Go out bytes, Go ns, Rust out bytes, Rust
# ns, and with the v4 builds Go v4 out bytes, Go v4 ns, Rust v4 out bytes, Rust v4 ns. ns per byte is
# ms per MB. Returns 1 when any default ratio is over 1.10 or any output size differs (the v4
# ratios are not gated).
table() {
  bad=0
  echo "### $1"
  echo
  if [ "$v4" = 1 ]; then
    # -a 1 -e 0 keeps every default row; a row the v4 build lacks gets zeros (v4 ns 0 is "v4 MISSING").
    join -t "$tab" "$2" "$3" | join -t "$tab" -a 1 -e 0 -o 0,1.2,1.3,1.4,1.5,2.2,2.3 - "$4" | join -t "$tab" -a 1 -e 0 -o 0,1.2,1.3,1.4,1.5,1.6,1.7,2.2,2.3 - "$5" >/tmp/joined.tsv
  else
    join -t "$tab" "$2" "$3" >/tmp/joined.tsv
  fi
  awk -F'\t' -v v4="$v4" '
function add(k) { b[k] += $2; g[k] += $3; r[k] += $5; c[k]++; if (v4) { g4[k] += $7; r4[k] += $9 } }
{
  k = "other"
  if (match($1, /none|lzw|packbits|deflate|jpeg|ccitt/)) k = substr($1, RSTART, RLENGTH)
  add(k); add("all")
}
END {
  s = "| input | files | MB out | Go ms/MB | Rust ms/MB | Rust / Go |"; t = "|---|---|---|---|---|---|"
  if (v4) { s = s " Go v4 ms/MB | Rust v4 ms/MB | v4 Rust / Go |"; t = t "---|---|---|" }
  print s " |"; print t "---|"
  m = split("none lzw packbits deflate jpeg ccitt other all", ks, " ")
  for (i = 1; i <= m; i++) {
    k = ks[i]
    if (b[k]) {
      s = sprintf("| %s | %d | %.2f | %.3f | %.3f | %.2f |", k, c[k], b[k] / 1e6, g[k] / b[k], r[k] / b[k], r[k] / g[k])
      if (v4) s = s sprintf(" %.3f | %.3f | %.2f |", g4[k] / b[k], r4[k] / b[k], g4[k] > 0 ? r4[k] / g4[k] : 0)
      print s " " (r[k] / g[k] > 1.10 ? "OVER 1.10" : "") " |"
      if (r[k] / g[k] > 1.10) bad = 1
    }
  }
  exit bad
}' /tmp/joined.tsv || bad=1
  # Per file, worst first: a ratio over 1.10 is marked, so a slow input is not hidden in its
  # compression's row above. Also on stderr, for reading the log.
  echo
  s="| file | out MB | Go ms/MB | Rust ms/MB | Rust / Go |"; t="|---|---|---|---|---|"
  if [ "$v4" = 1 ]; then s="$s Go v4 ms/MB | Rust v4 ms/MB | v4 Rust / Go |"; t="$t---|---|---|"; fi
  echo "$s |"
  echo "$t---|"
  awk -F'\t' '($2 > 0 || $4 > 0) && $3 > 0 { printf "%s\t%d\t%d\t%d\t%.2f\t%d\t%d\t%d\t%d\t%d\n", $1, $2, $3, $5, $5 / $3, $4, $7, $9, $6, $8 }' /tmp/joined.tsv | sort -t"$tab" -k5 -nr | tee /dev/stderr |
    awk -F'\t' -v v4="$v4" '{
      mark = ""
      if ($5 > 1.10) mark = "OVER 1.10"
      if ($2 != $6) { mark = mark " BYTES DIFFER (Rust " $6 ")"; bad = 1 }
      if (v4 && ($7 == 0 || $8 == 0)) { mark = mark " v4 MISSING"; bad = 1 }
      else if (v4 && ($9 != $2 || $10 != $6)) { mark = mark " v4 BYTES DIFFER (Go " $9 ", Rust " $10 ")"; bad = 1 }
      d = $2 > 0 ? $2 : 1
      s = sprintf("| %s | %.2f | %.3f | %.3f | %.2f |", $1, $2 / 1e6, $3 / d, $4 / d, $5)
      if (v4) s = s sprintf(" %.3f | %.3f | %.2f |", $7 / d, $8 / d, $7 > 0 ? $8 / $7 : 0)
      print s " " mark " |"
    }
    END { exit bad }' || bad=1
  echo
  return $bad
}

rc=0
if [ "$v4" = 1 ]; then
  table "Rust default max_threads against Go default" /tmp/go-def.tsv /tmp/rs-def.tsv /tmp/gov4-def.tsv /tmp/rsv4-def.tsv || rc=1
  table "Rust max_threads 1 against Go GOMAXPROCS=1" /tmp/go-one.tsv /tmp/rs-one.tsv /tmp/gov4-one.tsv /tmp/rsv4-one.tsv || rc=1
else
  table "Rust default max_threads against Go default" /tmp/go-def.tsv /tmp/rs-def.tsv || rc=1
  table "Rust max_threads 1 against Go GOMAXPROCS=1" /tmp/go-one.tsv /tmp/rs-one.tsv || rc=1
fi
# Diagnostic, not part of the gate: for jpeg-grey-strip, is a gap inside the JPEG decoder (raw) or
# around it (full)? Both at one thread.
GOMAXPROCS=1 oracle -benchraw 50 $d/jpeg-grey-strip.tif | sed 's/^/raw1: /' || true
$bench --raw $d/jpeg-grey-strip.tif 50 | sed 's/^/raw1: /' || true
# Not a gate: serving a JPEG tile as it is (Page::raw_jpeg_into) against decoding it, per tile.
$bench --rawtile $d/jpeg-rgb-tile.tif 50 || true
if [ "$v4" = 1 ]; then $bench4 --rawtile $d/jpeg-rgb-tile.tif 50 | sed 's/^/v4 /' || true; fi
exit $rc
