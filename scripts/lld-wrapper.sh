#!/bin/bash
# Wrapper for lld-link that adds Rust's self-contained libraries

SELF_CONTAINED="C:/Users/jiehu/.rustup/toolchains/stable-x86_64-pc-windows-gnu/lib/rustlib/x86_64-pc-windows-gnu/lib/self-contained"
LLVM_BIN="C:/Program Files/LLVM/bin"

exec "$LLVM_BIN/lld-link.exe" -L"$SELF_CONTAINED" "$@"