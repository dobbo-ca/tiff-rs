#!/bin/sh
# Times the bench example built three ways against Go: tests/parity/pgo.sh [N]. Needs `oracle` (Go)
# and `bench` (the default build), `bench-pgo` (cargo-pgo, trained on /bench-in) and, when BOLT was
# available, `bench-bolt` (PGO then BOLT) on PATH; run from the repo root (stage `pgo` of
# .github/ci/Dockerfile, workflow pgo.yml). Each build runs tests/parity/bench.sh (its Rust binary
# is $BENCH); the table gives the Rust ms/MB of each per class, default threads and then
# single-threaded (the order of bench.sh's tables), and the change against the default build.
set -eu
export LC_ALL=C
n=${1:-20}
t=$(mktemp -d)
have=
labels=
for v in bench:default bench-pgo:PGO bench-bolt:"PGO + BOLT"; do
  b=${v%%:*}
  if ! command -v "$b" >/dev/null 2>&1; then
    echo >&2 "no $b: skipped"
    continue
  fi
  # bench.sh exits 1 when its gate fails; the timings are still wanted.
  BENCH=$b sh tests/parity/bench.sh "$n" >"$t/$b.md" || echo >&2 "bench.sh $b: gate failed (timings still reported)"
  have="$have $t/$b.md"
  labels="$labels,${v#*:}"
done
[ -n "$have" ] || { echo >&2 "no bench binary on PATH"; exit 1; }
# Rust ms/MB is column 6 of a class row; a pass ends with its "all" row.
# shellcheck disable=SC2086
awk -F'|' -v labels="${labels#,}" '
FNR == 1 { nf++; alls = 0 }
/^[|] [a-z0-9-]+ [|] [0-9]+ [|]/ {
  name = $2; gsub(/ /, "", name)
  p = alls + 1
  v[nf, p, name] = $6 + 0
  if (!((p, name) in seen)) { seen[p, name] = 1; names[p, ++cnt[p]] = name }
  if (name == "all") alls++
}
END {
  split(labels, lab, ",")
  split("default threads,single-threaded", pass, ",")
  for (p = 1; p <= 2; p++) {
    print "#### Rust ms/MB, " pass[p] " (lower is better; the change is against the default build)"
    print ""
    s = "| class |"; r = "|---|"
    for (f = 1; f <= nf; f++) { s = s " " lab[f] " |"; r = r "---|"; if (f > 1) { s = s " change |"; r = r "---|" } }
    print s; print r
    for (i = 1; i <= cnt[p]; i++) {
      k = names[p, i]
      s = "| " k " |"
      for (f = 1; f <= nf; f++) {
        s = s sprintf(" %.3f |", v[f, p, k])
        if (f > 1 && v[1, p, k] > 0) s = s sprintf(" %+.1f%% |", (v[f, p, k] / v[1, p, k] - 1) * 100)
      }
      print s
    }
    print ""
  }
}' $have
for f in $have; do
  echo "<details><summary>$(basename "$f" .md): the full bench.sh tables</summary>"
  echo
  cat "$f"
  echo
  echo "</details>"
done
rm -rf "$t"
