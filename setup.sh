#!/usr/bin/env bash
# setup.sh — link the sixteen flex names into ~/.local/bin (OPT-10).
#
# Usage:
#   ./setup.sh          create (or refresh) the sixteen symlinks
#   ./setup.sh --check  assert all sixteen resolve to the single flex binary
#
# Single-binary install: every `~/.local/bin/flex-*` name is a symlink to
# `./target/release/flex` (build with `cargo build --release` first). The
# `flex` binary resolves `argv[0]` (`flex-power`, …) or `flex <provider>`
# and re-execs the matching sibling `target/release/flex-<provider>`
# binary, so all sixteen names keep working while the installed farm is
# one binary plus shims. `--check` is the CI gate for that layout.

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
    flex-bt
    flex-notify
    flex-profile
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

# The single binary every farm entry must resolve to.
EXPECTED="$(readlink -f "$SRC_DIR/flex" 2>/dev/null || true)"

if [[ "$check_mode" == "1" ]]; then
    missing=0
    if [[ ! -x "$SRC_DIR/flex" ]]; then
        echo "setup.sh: missing single binary: $SRC_DIR/flex" >&2
        missing=1
    fi
    for name in "${BINS[@]}"; do
        link="$BIN_DIR/$name"
        if [[ ! -L "$link" || ! -x "$link" ]]; then
            echo "setup.sh: missing executable: $link" >&2
            missing=1
            continue
        fi
        resolved="$(readlink -f "$link" 2>/dev/null || true)"
        if [[ "$resolved" != "$EXPECTED" ]]; then
            echo "setup.sh: $link resolves to $resolved, not $EXPECTED" >&2
            missing=1
        fi
    done
    if [[ "$missing" != "0" ]]; then
        exit 1
    fi
    echo "setup.sh: all ${#BINS[@]} names resolve to the single binary $EXPECTED"
    exit 0
fi

mkdir -p "$BIN_DIR"
if [[ ! -x "$SRC_DIR/flex" ]]; then
    fail "release binary not found: $SRC_DIR/flex (run \`cargo build --release\` first)"
fi
for name in "${BINS[@]}"; do
    ln -sfn "$SRC_DIR/flex" "$BIN_DIR/$name"
done
echo "setup.sh: linked ${#BINS[@]} names to the single binary in $BIN_DIR"
