@echo off
REM Build in release profile and run
setlocal
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0dev.ps1" -Release %*
endlocal
