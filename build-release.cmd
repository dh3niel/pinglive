@echo off
rem Builds the release exe, installs it as the PingLive service (which starts
rem the overlay at every sign-in) and puts a shortcut on the desktop.
rem Double-click it or run it from a terminal; approve the admin prompt.
setlocal
cd /d "%~dp0"

where cargo >nul 2>nul
if errorlevel 1 (
    echo cargo was not found. Install Rust from https://rustup.rs first.
    goto :fail
)

echo [1/3] Building the release exe...
cargo build --release
if errorlevel 1 goto :fail

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\install-release.ps1"
if errorlevel 1 goto :fail

echo.
pause
exit /b 0

:fail
echo.
echo Failed - see the message above.
pause
exit /b 1
