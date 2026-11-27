#!/usr/bin/env bash
# Builds crates/link-window-ffi for the window and copies the static library to OUT: cargo_ffi.sh OUT [TARGET]
# CARGO_TARGET_DIR picks cargo's build dir (default: the repo's target/), so a rebuild reuses the crates.
set -euo pipefail
out=$1 target=${2:-}
repo=$(realpath "$(dirname "$0")/..")
dir=${CARGO_TARGET_DIR:-$repo/target}
args=(build --release --locked -q -p link-window-ffi --manifest-path "$repo/Cargo.toml" --target-dir "$dir")
[ -z "$target" ] || args+=(--target "$target")
cargo "${args[@]}"
cp "$dir/${target:+$target/}release/liblink_window_ffi.a" "$out"
