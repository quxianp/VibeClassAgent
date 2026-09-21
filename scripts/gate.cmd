@echo off
REM ===================================================================
REM VibeClassAgent - quality gate (CMD entry point)
REM -------------------------------------------------------------------
REM Runs, in order: cargo fmt --check -> clippy -D warnings -> test.
REM Uses the same toolchain setup as dev.cmd (shared scripts\env.ps1),
REM so the two never disagree about CC / PATH / as.exe.
REM
REM USAGE:
REM   scripts\gate.cmd                 all three
REM   scripts\gate.cmd -SkipTest       format + clippy only
REM   scripts\gate.cmd -SkipFmt        clippy + test only
REM ===================================================================
setlocal
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0gate.ps1" %*
endlocal
