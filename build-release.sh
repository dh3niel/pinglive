#!/usr/bin/env bash
# Builds the release exe, installs it as the PingLive service (which starts
# the overlay at every sign-in) and puts a shortcut on the desktop.
# Run from Git Bash on Windows; approve the admin prompt when it appears.
set -euo pipefail
cd "$(dirname "$0")"

if ! command -v cargo >/dev/null; then
    echo "cargo was not found. Install Rust from https://rustup.rs first." >&2
    exit 1
fi

echo "[1/3] Building the release exe..."
cargo build --release

powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/install-release.ps1
