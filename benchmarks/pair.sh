#!/bin/sh
# Compares two builds of a benchmark, alternating single runs of each (the order swapped every pair), because the
# machine slows under sustained load and runs of one build in a row would all land on the slower part.
# Usage: benchmarks/pair.sh processors pairs exe_a exe_b [NAME=value ...]
# Runs each with COOPER_THREADS=processors and the given environment; prints the minimum and median wall time of
# each, and the median of b/a over the pairs.
n=$1; pairs=$2; a=$3; b=$4; shift 4
err=$(mktemp); trap 'rm -f "$err"' EXIT
one() { /usr/bin/time -p env COOPER_THREADS="$n" "$@" >/dev/null 2>"$err"; awk '/^real/{print $2}' "$err"; }
ta=""; tb=""; ratios=""
for i in $(seq "$pairs"); do
  if [ $((i % 2)) = 1 ]; then x=$(one "$@" "$a"); y=$(one "$@" "$b"); else y=$(one "$@" "$b"); x=$(one "$@" "$a"); fi
  ta="$ta $x"; tb="$tb $y"; ratios="$ratios $(echo "$y $x" | awk '{printf "%.4f", $1/$2}')"
done
stat() { echo $1 | tr ' ' '\n' | sort -n | awk '{v[NR]=$1} END {printf "min %.3f median %.3f", v[1], v[int((NR+1)/2)]}'; }
med() { echo $1 | tr ' ' '\n' | sort -n | awk '{v[NR]=$1} END {printf "%+.1f%%", (v[int((NR+1)/2)]-1)*100}'; }
echo "$(basename "$a"): $(stat "$ta")   $(basename "$b"): $(stat "$tb")   b/a median $(med "$ratios")"
