#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DATA_DIR="$ROOT/.demo-data"

if ! command -v cargo >/dev/null 2>&1; then
  echo "ERROR: cargo is required." >&2
  exit 1
fi
if ! command -v gradle >/dev/null 2>&1; then
  echo "ERROR: gradle is required." >&2
  exit 1
fi

JAVA_MAJOR="$(java -version 2>&1 | sed -n 's/.*version "\([0-9][0-9]*\).*/\1/p' | head -n1)"
if [[ -z "$JAVA_MAJOR" || "$JAVA_MAJOR" -lt 22 ]]; then
  echo "ERROR: JDK 22+ is required by the finalized Java FFM API." >&2
  exit 1
fi

case "$(uname -s)" in
  Linux)  LIB="$ROOT/target/release/libadb_ffi.so" ;;
  Darwin) LIB="$ROOT/target/release/libadb_ffi.dylib" ;;
  *) echo "ERROR: demo script currently supports Linux and macOS." >&2; exit 1 ;;
esac

printf 'Building native engine...\n'
(cd "$ROOT" && cargo build --release -p adb-ffi)

printf 'Resetting repo-local demo database: %s\n' "$DATA_DIR"
rm -rf -- "$DATA_DIR"
mkdir -p "$DATA_DIR"

export ADB_DATA="$DATA_DIR"
export ADB_NATIVE_LIBRARY="$LIB"

printf 'Running JVM feature tour...\n'
(cd "$ROOT/jvm" && gradle :adb-cli:run --args='--demo')
