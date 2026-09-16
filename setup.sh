#!/usr/bin/env bash
# setup.sh — link the thirteen flex release binaries into ~/.local/bin.
#
# Usage:
#   ./setup.sh          create (or refresh) the thirteen symlinks
#   ./setup.sh --check  assert all thirteen resolve into the current target/release
#
# The links point at ./target/release/ (build with `cargo build --release`
# first); `--check` is the CI gate that the dispatcher and the twelve
# per-provider/helper binaries are all installed.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
SRC_DIR="$ROOT/target/release"
BIN_DIR="${HOME:-}/.local/bin"

BINS=(
    flex
    flex-power
    flex-launch
    flex-shot
    flex-theme
    flex-clip
    flex-center
    flex-wallpaper
    flex-wifi
    flex-proc
    flex-record
    flex-mixer
    flex-net
)

usage() {
    echo "usage: $0 [--check]" >&2
}

fail() {
    echo "setup.sh: error: $1" >&2
    exit 1
}

if [[ "${HOME:-}" == "" ]]; then
    fail "HOME is unset, cannot locate ~/.local/bin"
fi

check_mode=0
if [[ "${1:-}" == "--check" ]]; then
    check_mode=1
elif [[ "${1:-}" != "" ]]; then
    usage
    exit 2
fi

if [[ "$check_mode" == "1" ]]; then
    missing=0
    for name in "${BINS[@]}"; do
        link="$BIN_DIR/$name"
        expected="$(readlink -f "$SRC_DIR/$name" 2>/dev/null || true)"
        if [[ ! -L "$link" || ! -x "$link" ]]; then
            echo "setup.sh: missing executable: $link" >&2
            missing=1
            continue
        fi
        resolved="$(readlink -f "$link" 2>/dev/null || true)"
        if [[ "$resolved" != "$expected" ]]; then
            echo "setup.sh: $link resolves to $resolved, not $expected" >&2
            missing=1
        fi
    done
    if [[ "$missing" != "0" ]]; then
        exit 1
    fi
    echo "setup.sh: all ${#BINS[@]} binaries resolve to executables in $BIN_DIR"
    exit 0
fi

mkdir -p "$BIN_DIR"
for name in "${BINS[@]}"; do
    src="$SRC_DIR/$name"
    if [[ ! -x "$src" ]]; then
        fail "release binary not found: $src (run \`cargo build --release\` first)"
    fi
    ln -sfn "$src" "$BIN_DIR/$name"
done
echo "setup.sh: linked ${#BINS[@]} binaries into $BIN_DIR"
