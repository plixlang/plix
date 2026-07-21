#!/usr/bin/env bash
# Release preflight for a locally built Plix binary.
# Usage: bash tests/release_preflight.sh [path-to-plix]
set -euo pipefail
cd "$(dirname "$0")/.."

PLIX="${1:-./target/release/plix}"
if [[ ! -x "$PLIX" ]]; then
  echo "error: Plix executable not found or not executable: $PLIX" >&2
  exit 2
fi

version_from() {
  awk -F '"' '/^version =/ { print $2; exit }' "$1"
}
TOOLCHAIN_VERSION="$(version_from Cargo.toml)"
RUNTIME_VERSION="$(version_from rt/Cargo.toml)"
if [[ -z "$TOOLCHAIN_VERSION" || "$TOOLCHAIN_VERSION" != "$RUNTIME_VERSION" ]]; then
  echo "error: package versions differ (plix=$TOOLCHAIN_VERSION, plixrt=$RUNTIME_VERSION)" >&2
  exit 1
fi
if ! grep -Fq "plix $TOOLCHAIN_VERSION (rust runtime)" rt/src/builtins.rs; then
  echo "error: embedded runtime version string does not match $TOOLCHAIN_VERSION" >&2
  exit 1
fi

reported="$($PLIX --version)"
if [[ "$reported" != "plix $TOOLCHAIN_VERSION" ]]; then
  echo "error: CLI version mismatch: expected 'plix $TOOLCHAIN_VERSION', got '$reported'" >&2
  exit 1
fi

echo "== version consistency: $TOOLCHAIN_VERSION =="
echo "== release binary test suites =="
"$PLIX" test tests
echo "== dual-mode parity battery =="
PLIX_BIN_DIR="${PLIX_BIN_DIR:-$PWD/target/release-examples}" bash tests/run_all.sh "$PLIX"
echo "== deterministic fuzz parity (40 seeds) =="
bash tests/fuzz_parity.sh 40 "$PLIX"
echo "== release preflight passed: v$TOOLCHAIN_VERSION =="
