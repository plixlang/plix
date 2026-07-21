#!/usr/bin/env sh
# Install a Plix release archive after extracting it.
set -eu

PREFIX="${PREFIX:-$HOME/.local}"
SOURCE_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
BINARY="$SOURCE_DIR/bin/plix"
if [ ! -f "$BINARY" ]; then
  echo "error: expected $BINARY; run this script from an extracted Plix archive" >&2
  exit 1
fi
mkdir -p "$PREFIX/bin"
install -m 755 "$BINARY" "$PREFIX/bin/plix"
printf 'Installed Plix to %s/bin/plix\n' "$PREFIX"
printf 'Ensure %s/bin is on your PATH, then run: plix --version\n' "$PREFIX"
