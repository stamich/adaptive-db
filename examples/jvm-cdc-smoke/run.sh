#!/usr/bin/env bash
# Builds the native library, compiles the Java FFM binding with plain javac (no Maven/Gradle
# needed) and runs the CDC end-to-end smoke test against it.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="$(mktemp -d)"
trap 'rm -rf "$OUT"' EXIT

case "$(uname -s)" in
  Linux)  LIB="$ROOT/target/release/libadb_ffi.so" ;;
  Darwin) LIB="$ROOT/target/release/libadb_ffi.dylib" ;;
  *) echo "unsupported OS" >&2; exit 1 ;;
esac

(cd "$ROOT" && cargo build --release -p adb-ffi)
javac -d "$OUT" "$ROOT"/jvm/adb-native/src/main/java/io/adb/ffm/*.java
javac -cp "$OUT" -d "$OUT" "$ROOT/examples/jvm-cdc-smoke/CdcSmoke.java"
java --enable-native-access=ALL-UNNAMED -cp "$OUT" CdcSmoke "$LIB"
