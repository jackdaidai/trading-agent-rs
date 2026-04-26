@echo off
set SELF_CONTAINED=C:\Users\jiehu\.rustup\toolchains\stable-x86_64-pc-windows-gnu\lib\rustlib\x86_64-pc-windows-gnu\lib\self-contained
set LOCAL_LIB=%SELF_CONTAINED%
"C:\Program Files\LLVM\bin\lld-link.exe" -LIBPATH:"%LOCAL_LIB%" %*
