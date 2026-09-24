<#
.SYNOPSIS
    Stops the PingLive overlay and removes its logon task.
#>
[CmdletBinding()]
param([string]$TaskName = 'PingLive Overlay')

$ErrorActionPreference = 'Stop'

if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
    Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
    Write-Host "Removed scheduled task '$TaskName'."
} else {
    Write-Host "No scheduled task named '$TaskName'."
}

Get-Process -Name pinglive -ErrorAction SilentlyContinue | Stop-Process
Write-Host 'Overlay stopped.'
