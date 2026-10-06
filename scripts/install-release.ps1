<#
.SYNOPSIS
    Installs a built pinglive.exe as the PingLive service and adds a desktop shortcut.

.DESCRIPTION
    Called by build-release.cmd / build-release.sh after `cargo build --release`.
    `pinglive.exe --install` copies the exe to Program Files and registers the
    auto-start service that launches the overlay at every sign-in. That needs
    administrator rights, so Windows shows a UAC prompt.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\install-release.ps1
#>
[CmdletBinding()]
param(
    # Path to the built exe. Defaults to the release build in this repo.
    [string]$ExePath = (Join-Path $PSScriptRoot '..\target\release\pinglive.exe')
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $ExePath)) {
    throw "Executable not found: $ExePath. Build it first with: cargo build --release"
}
$ExePath = (Resolve-Path -LiteralPath $ExePath).Path

Write-Host '[2/3] Installing the service - approve the admin prompt, then click OK on the message...'
Start-Process -FilePath $ExePath -ArgumentList '--install' -Verb RunAs -Wait

# The installer reports through a message box, not an exit code, so check
# that this build is really the one in Program Files now.
$programFiles = if ($env:ProgramW6432) { $env:ProgramW6432 } else { $env:ProgramFiles }
$installed = Join-Path $programFiles 'PingLive\pinglive.exe'
if (-not (Test-Path -LiteralPath $installed) -or
    (Get-FileHash -LiteralPath $installed).Hash -ne (Get-FileHash -LiteralPath $ExePath).Hash) {
    throw "The install did not complete: $installed is not this build."
}
$service = Get-Service -Name 'PingLive' -ErrorAction SilentlyContinue
if (-not $service) {
    throw 'The PingLive service was not registered.'
}
if ($service.Status -ne 'Running') {
    Write-Warning "The PingLive service is $($service.Status), not Running."
}

Write-Host '[3/3] Creating the desktop shortcut...'
$link = Join-Path ([Environment]::GetFolderPath('Desktop')) 'PingLive.lnk'
$shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($link)
$shortcut.TargetPath = $installed
# Opens the dashboard of the overlay that is already running, or starts it.
$shortcut.Arguments = '--dashboard'
$shortcut.WorkingDirectory = Split-Path $installed
$shortcut.IconLocation = "$installed,0"
$shortcut.Description = 'PingLive ping overlay'
$shortcut.Save()

Write-Host "Done. PingLive is installed in $(Split-Path $installed), starts at every sign-in,"
Write-Host "and has a shortcut on the desktop: $link"
