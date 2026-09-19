@echo off
REM Environment self-check (CMD entry, bypasses .ps1 execution policy)
setlocal
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0doctor.ps1"
endlocal
