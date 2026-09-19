@echo off
REM ===================================================================
REM VibeClassAgent - dev launcher (CMD entry point)
REM -------------------------------------------------------------------
REM WHY: Windows blocks .ps1 by default (Restricted execution policy).
REM This wrapper calls PowerShell with -ExecutionPolicy Bypass, which
REM affects only this process and does NOT change system settings.
REM
REM NOTE ON ARGUMENTS: pass them through positionally (%*), never as
REM "-Args %*". With no arguments that expands to a bare "-Args" and
REM PowerShell fails with "Missing an argument for parameter 'Args'".
REM
REM USAGE:
REM   scripts\dev.cmd                       build and enter interactive UI
REM   scripts\dev.cmd doctor                environment self-check
REM   scripts\dev.cmd debug e2e             end-to-end smoke test
REM   scripts\dev.cmd run --dry-run         daemon dry-run
REM   scripts\dev.cmd -Check                cargo check only
REM ===================================================================
setlocal
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0dev.ps1" %*
endlocal
