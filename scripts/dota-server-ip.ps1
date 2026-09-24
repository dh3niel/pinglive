<#
.SYNOPSIS
    Shows the remote IPs dota2.exe is currently talking to.

.DESCRIPTION
    Dota 2 uses UDP, so there is no "connection" in the TCP sense. This walks
    the UDP endpoints owned by dota2.exe and, when the game is in a match,
    also shows the Source relay addresses seen in netstat. Feed the one that
    keeps appearing during a match into the overlay:

        pinglive.exe --target <ip>

    or set `target` in %APPDATA%\PingLive\config.toml.
#>
$proc = Get-Process -Name dota2 -ErrorAction SilentlyContinue
if (-not $proc) {
    Write-Host 'dota2.exe is not running - start a match first.'
    return
}

Write-Host "dota2.exe PID: $($proc.Id)`n"

$rows = netstat -ano -p UDP | Select-String "\s$($proc.Id)$"
if (-not $rows) {
    Write-Host 'No UDP endpoints found (netstat does not show UDP peers for all sockets).'
} else {
    $rows | ForEach-Object { $_.Line.Trim() }
}

Write-Host "`nTCP peers (Steam / GC):"
Get-NetTCPConnection -OwningProcess $proc.Id -State Established -ErrorAction SilentlyContinue |
    Select-Object RemoteAddress, RemotePort |
    Sort-Object RemoteAddress -Unique |
    Format-Table -AutoSize
