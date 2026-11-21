#!/usr/bin/env bash
# Native vs vanilla worldgen benchmark + end-to-end correctness check.
#
# Runs N pairs of headless 1.18.2 servers (same seed, same 625-chunk area, fresh worlds),
# alternating which side goes first to cancel out ordering/thermal effects. Each run
# generates the area to the NOISE stage and writes per-chunk hashes; every pair's hashes
# must be identical.
#
# Usage:  scripts/pregen-bench.sh [pairs=3]
# Needs:  JDK 25 for Gradle/Loom (set JAVA_HOME), eula.txt accepted in run/pregen-*/.
# Output: build/pregen-bench/summary.txt (+ full server logs next to it)
#
# For cleaner numbers: close the game/browser, and keep the machine otherwise idle.
set -uo pipefail
cd "$(dirname "$0")/.."

PAIRS="${1:-3}"
OUT=build/pregen-bench
mkdir -p "$OUT"
SUMMARY="$OUT/summary.txt"
echo "# pregen benchmark $(date -Is), $PAIRS pairs, $(nproc) cores, $(free -g | awk '/Mem/{print $2}') GB RAM" > "$SUMMARY"

for d in run/pregen-native run/pregen-vanilla; do
  if ! grep -qs "eula=true" "$d/eula.txt"; then
    echo "error: $d/eula.txt must contain eula=true (Minecraft EULA: https://aka.ms/MinecraftEULA)" >&2
    exit 1
  fi
done

run_one() { # $1 = Native | Vanilla, $2 = pair index
  local mode=$1 i=$2 dir="run/pregen-${1,,}" log="$OUT/$1-$2.log"
  rm -rf "$dir/world" "$dir/rustgen-pregen.txt" "$dir/rustgen-fills.txt"
  timeout 600 ./gradlew "runPregen$mode" --console=plain > "$log" 2>&1
  # The server JVM is forked by the Gradle daemon, so a timeout doesn't stop it; make
  # sure no server is left holding the port (its command line carries -Drustgen.pregen=).
  local left
  left=$(pgrep -f -- '-Drustgen\.pregen=' || true)
  if [ -n "$left" ]; then
    # Still running after the timeout: record where it is stuck, then kill it.
    for pid in $left; do
      jstack "$pid" > "$OUT/$mode-$i.jstack.txt" 2>&1 || true
      sleep 20
      jstack "$pid" > "$OUT/$mode-$i.jstack2.txt" 2>&1 || true
    done
    kill -9 $left 2>/dev/null
    echo "pair $i  $mode  (timed out; thread dump in $OUT/$mode-$i.jstack.txt)" | tee -a "$SUMMARY"
  fi
  local rate fill
  rate=$(grep -oE '[0-9]+,?[0-9]* chunks/s' "$log" | tail -1)
  fill=$(grep -oE 'doFill [0-9.,]+ ms/chunk' "$log" | tail -1)
  echo "pair $i  $mode  ${rate:-FAILED}  ${fill:-}" | tee -a "$SUMMARY"
}

for i in $(seq 1 "$PAIRS"); do
  if (( i % 2 )); then order="Native Vanilla"; else order="Vanilla Native"; fi
  for m in $order; do run_one "$m" "$i"; done
  # Every doFill output (spawn area included), compared by chunk position.
  if [ -f run/pregen-native/rustgen-fills.txt ] && [ -f run/pregen-vanilla/rustgen-fills.txt ]; then
    cp run/pregen-native/rustgen-fills.txt "$OUT/Native-$i.fills.txt"
    cp run/pregen-vanilla/rustgen-fills.txt "$OUT/Vanilla-$i.fills.txt"
    join <(awk '{print $1"_"$2, $3}' run/pregen-native/rustgen-fills.txt | sort) \
         <(awk '{print $1"_"$2, $3}' run/pregen-vanilla/rustgen-fills.txt | sort) \
      | awk -v i="$i" '{n++; if ($2 != $3) {d++; if (d <= 5) print "pair " i "  FILL DIFF at chunk " $1}} END {print "pair " i "  doFill hashes: " n " common chunks, " d+0 " differ"}' \
      | tee -a "$SUMMARY"
  fi
  if cmp -s run/pregen-native/rustgen-pregen.txt run/pregen-vanilla/rustgen-pregen.txt; then
    echo "pair $i  hashes IDENTICAL ($(wc -l < run/pregen-native/rustgen-pregen.txt) chunks)" | tee -a "$SUMMARY"
  else
    echo "pair $i  HASH MISMATCH - native output differs from vanilla!" | tee -a "$SUMMARY"
  fi
done
echo "DONE" >> "$SUMMARY"
