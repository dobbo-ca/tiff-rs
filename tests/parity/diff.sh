#!/bin/sh
# tests/parity/diff.sh <go-out> <rust-out>: exits 1 if the trees differ, after listing the first
# differences and the first three differing files.
set -eu
if diff -rq "$1" "$2" >/tmp/diff.txt; then
  exit 0
fi
head -50 /tmp/diff.txt
wc -l /tmp/diff.txt
for f in $(sed -n 's/^Files \([^ ]*\) and .*/\1/p' /tmp/diff.txt | head -3); do
  b=$(basename "$f")
  echo "== $b"
  diff "$f" "$2/$b" | head -10
done
exit 1
