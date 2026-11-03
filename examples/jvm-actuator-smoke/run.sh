#!/usr/bin/env bash
# Builds the native library, compiles the Java FFM binding and the Scala 3 JVM modules with plain
# javac/scalac (no Gradle needed; `scalac` and `scala` from a Scala 3.3 distribution must be on
# PATH) and runs the actuator end-to-end smoke test against the real engine.
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
# shellcheck disable=SC2046
scalac -d "$OUT" -classpath "$OUT" $(find "$ROOT"/jvm -path '*/src/main/scala/*.scala' -not -path '*/adb-cli/*' -not -path '*/adb-benchmark/*') \
  "$ROOT/examples/jvm-actuator-smoke/ActuatorSmoke.scala"
JAVA_OPTS="--enable-native-access=ALL-UNNAMED" scala -classpath "$OUT" ActuatorSmoke "$LIB"
