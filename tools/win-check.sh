#!/usr/bin/env bash
# Cross-checks the crates that already build for Windows (TASK-link-windows W0). Add a crate here once it compiles;
# a Linux-only call in one of them then fails `just win-check` and the windows workflow.
# Needs: rustup target x86_64-pc-windows-gnullvm, clang, llvm-ar, MinGW-w64 headers (Arch: mingw-w64-headers,
# Ubuntu: mingw-w64-x86-64-dev) for ring's C code.
set -euo pipefail
cd "$(dirname "$0")/.."

crates=(link-proto link-quickshare)
if [ "${1:-}" = --list ]; then printf '%s\n' "${crates[@]}"; exit 0; fi

target=x86_64-pc-windows-gnullvm
sysroot=${MINGW_SYSROOT:-/usr/x86_64-w64-mingw32}
export CC_x86_64_pc_windows_gnullvm=clang
export CFLAGS_x86_64_pc_windows_gnullvm="--target=x86_64-w64-windows-gnu --sysroot=$sysroot"
export AR_x86_64_pc_windows_gnullvm=llvm-ar

args=()
for c in "${crates[@]}"; do args+=(-p "$c"); done
cargo clippy --target "$target" "${args[@]}" --all-targets -- -D warnings
