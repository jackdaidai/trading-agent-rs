#!/bin/bash
# Wrapper for lld-link that adds the correct library search path

SELF_CONTAINED="C:/Users/jiehu/.rustup/toolchains/stable-x86_64-pc-windows-gnu/lib/rustlib/x86_64-pc-windows-gnu/lib/self-contained"

exec "C:/Program Files/LLVM/bin/lld-link.exe" -L"$SELF_CONTAINED" "$@"