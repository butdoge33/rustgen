#!/usr/bin/env bash
# Verifies the Windows build of the native library produces bit-identical results to the
# Linux build (which the Gradle parity tests verify against vanilla).
#
# Runs src/crosscheck's harness with librustgen.so on Linux and with rustgen.dll on a
# Windows JDK 17 under Wine, then compares the SHA-256 over ~6M output values.
#
# Needs: gcc-mingw-w64-x86-64, wine, rustup target x86_64-pc-windows-gnu, a Linux JDK 17+,
# and build/overworld-12345.{rgdf,mat} (written by ChunkParityTest.chunkThroughput).
# Downloads Temurin JDK 17 for Windows once into ~/.cache/rustgen-wintest.
set -euo pipefail
cd "$(dirname "$0")/.."
W=~/.cache/rustgen-wintest
JAVA=${JAVA:-/usr/lib/jvm/java-21-openjdk-amd64/bin}
mkdir -p "$W"

for f in build/overworld-12345.rgdf build/overworld-12345.mat; do
  [ -f "$f" ] || { echo "missing $f: run ./gradlew test --tests rustgen.ChunkParityTest.chunkThroughput first" >&2; exit 1; }
done

(cd native && cargo build --release -q && cargo build --release -q --target x86_64-pc-windows-gnu)

rm -rf build/crosscheck && mkdir -p build/crosscheck/classes
"$JAVA/javac" --release 17 -d build/crosscheck/classes src/crosscheck/java/rustgen/*.java \
  src/main/java/rustgen/NativeNoise.java src/main/java/rustgen/NativeDensity.java

if [ ! -x "$W/jdk17/bin/java.exe" ]; then
  echo "downloading Temurin JDK 17 (Windows x64) into $W ..."
  curl -sfL -o "$W/jdk17.zip" "https://api.adoptium.net/v3/binary/latest/17/ga/windows/x64/jdk/hotspot/normal/eclipse"
  (cd "$W" && unzip -q jdk17.zip && mv jdk-17* jdk17 && rm jdk17.zip)
fi
cp native/target/x86_64-pc-windows-gnu/release/rustgen.dll build/overworld-12345.rgdf build/overworld-12345.mat "$W/"
rm -rf "$W/classes" && cp -r build/crosscheck/classes "$W/classes"

linux=$("$JAVA/java" -cp build/crosscheck/classes rustgen.CrossCheck native/target/release/librustgen.so \
  build/overworld-12345.rgdf build/overworld-12345.mat)
windows=$(cd "$W" && WINEPREFIX="$W/wineprefix" WINEDEBUG=-all wine jdk17/bin/java.exe -cp classes \
  rustgen.CrossCheck rustgen.dll overworld-12345.rgdf overworld-12345.mat 2>/dev/null | tr -d '\r')
echo "Linux:   $(echo "$linux" | tr '\n' ' ')"
echo "Windows: $(echo "$windows" | tr '\n' ' ')"
if [ "$(echo "$linux" | grep digest)" = "$(echo "$windows" | grep digest)" ]; then
  echo "IDENTICAL"
else
  echo "MISMATCH: the Windows library differs from the Linux library" >&2
  exit 1
fi
