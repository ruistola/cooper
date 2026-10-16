#!/bin/sh
# Times each benchmark in Cooper (unoptimized, and with clang -O2), Go, and Java: the best wall
# time of three runs and the peak memory. Usage: benchmarks/run.sh [name ...]
cd "$(dirname "$0")/.." || exit 1
cargo build -q --release 2>/dev/null || cargo build -q
COOPER=target/release/cooper; [ -x "$COOPER" ] || COOPER=target/debug/cooper
mkdir -p /tmp/cooper-bench
cat > /tmp/cooper-bench/cc-o2.sh <<'WRAP'
#!/bin/sh
if [ "$1" = "-print-target-triple" ]; then exec clang "$@"; fi
exec clang -O2 "$@"
WRAP
chmod +x /tmp/cooper-bench/cc-o2.sh
names="${*:-binary_trees live_heap strings}"
best() { # best of three wall times, and the peak resident set (MiB), of a command
  b=999999; rss=0
  for i in 1 2 3; do
    out=$( { /usr/bin/time -l "$@" >/dev/null; } 2>&1 )
    t=$(echo "$out" | awk '/ real /{print $1}')
    [ -z "$t" ] && t=$(echo "$out" | awk '/real/{print $1}')
    m=$(echo "$out" | awk '/maximum resident/{print $1}')
    b=$(echo "$t $b" | awk '{print ($1<$2)?$1:$2}')
    rss=$(echo "$m" | awk '{printf "%d", $1/1048576}')
  done
  echo "$b s, $rss MiB"
}
for n in $names; do
  echo "== $n"
  "$COOPER" build "benchmarks/$n.coop" >/dev/null && echo "cooper -O0: $(best "benchmarks/build/$n")"
  COOPER_CC=/tmp/cooper-bench/cc-o2.sh "$COOPER" build "benchmarks/$n.coop" >/dev/null && echo "cooper -O2: $(best "benchmarks/build/$n")"
  go build -o "/tmp/cooper-bench/$n-go" "benchmarks/$n.go" && echo "go:         $(best "/tmp/cooper-bench/$n-go")"
  class=$(echo "$n" | awk -F_ '{for(i=1;i<=NF;i++) printf toupper(substr($i,1,1)) substr($i,2)}')
  if javac -version >/dev/null 2>&1; then
    javac -d /tmp/cooper-bench "benchmarks/java/$class.java" && echo "java:       $(best java -cp /tmp/cooper-bench "$class")"
  fi
done
