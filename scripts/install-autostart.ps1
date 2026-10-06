<#
.SYNOPSIS
    Runs PingLive automatically at logon.

.DESCRIPTION
    A real Windows service cannot do this job: services run in session 0 and
    can never draw on your desktop, so an overlay started as a service would
    be invisible. The equivalent is a Scheduled Task that starts at logon in
    your own session, which is what this script registers. It restarts the
    overlay if it ever crashes and runs with no console window.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\install-autostart.ps1
#>
[CmdletBinding()]
param(
    # Path to the built exe. Defaults to the release build in this repo.
    [string]$ExePath = (Join-Path $PSScriptRoot '..\target\release\pinglive.exe'),
    [string]$TaskName = 'PingLive Overlay',
    # Optional extra args, e.g. '--target 8.8.8.8'
    [string]$Arguments = ''
)

$ErrorActionPreference = 'Stop'

$ExePath = (Resolve-Path -LiteralPath $ExePath).Path
if (-not (Test-Path -LiteralPath $ExePath)) {
    throw "Executable not found: $ExePath. Build it first with: cargo build --release"
}

# --autostart: wait out startup_delay_secs, then stay quiet until the first reply.
$Arguments = "--autostart $Arguments".Trim()
$action = New-ScheduledTaskAction -Execute $ExePath -Argument $Arguments -WorkingDirectory (Split-Path $ExePath)

$trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME

$settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -DontStopOnIdleEnd `
    -ExecutionTimeLimit ([TimeSpan]::Zero) `
    -RestartCount 3 `
    -RestartInterval (New-TimeSpan -Minutes 1) `
    -MultipleInstances IgnoreNew

$principal = New-ScheduledTaskPrincipal -UserId "$env:USERDOMAIN\$env:USERNAME" -LogonType Interactive -RunLevel Limited

Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger `
    -Settings $settings -Principal $principal -Force | Out-Null

Write-Host "Registered scheduled task '$TaskName' -> $ExePath"
Write-Host "Starting it now (it appears after the startup delay, 60 s by default)..."
Start-ScheduledTask -TaskName $TaskName
Write-Host "Done. Remove it again with scripts\uninstall-autostart.ps1"
