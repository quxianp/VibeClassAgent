@echo off
REM ===================================================================
REM VibeClassAgent - dev launcher (CMD entry point)
REM -------------------------------------------------------------------
REM WHY: Windows blocks .ps1 by default (Restricted execution policy).
REM This wrapper calls PowerShell with -ExecutionPolicy Bypass, which
REM affects only this process and does NOT change system settings.
REM
REM USAGE:
REM   scripts\dev.cmd                       show banner
REM   scripts\dev.cmd doctor                environment self-check
REM   scripts\dev.cmd overlay demo 8        show overlay for 8 seconds
REM   scripts\dev.cmd overlay plan          print today's overlay timing
REM   scripts\dev.cmd run --dry-run         daemon dry-run
REM   scripts\dev.cmd plugin list           list loaded plugins
REM   scripts\dev.cmd clean --dry-run       preview expired-recording cleanup
REM ===================================================================
setlocal
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0dev.ps1" -Args %*
endlocal
