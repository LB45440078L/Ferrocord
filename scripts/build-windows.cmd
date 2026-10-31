@echo off
rem Double-click this file to build all Ferrocord packages for Windows (and Linux via WSL2).
rem Options are passed on to build-windows.ps1, e.g.  build-windows.cmd -Arch x64 -SkipLinux
chcp 65001 >nul
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-windows.ps1" %*
set "code=%errorlevel%"
echo.
pause
exit /b %code%
