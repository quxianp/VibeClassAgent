@echo off
REM ===================================================================
REM VibeClassAgent - cargo fmt --all (in place)
REM -------------------------------------------------------------------
REM Uses the same toolchain setup as dev.cmd / gate.cmd (scripts\env.ps1);
REM plain `cargo fmt` would pick rustup's default toolchain and fail here.
REM ===================================================================
setlocal
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0fmt.ps1" %*
endlocal
