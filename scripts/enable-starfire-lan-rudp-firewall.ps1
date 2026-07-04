param(
  [int[]] $Ports = @(17870, 17871, 17874, 17886, 17887, 17888)
)

$ErrorActionPreference = "Stop"

$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).
  IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
  throw "Run this script from an elevated PowerShell session on Starfire."
}

foreach ($port in $Ports) {
  $name = "GameCult LAN RUDP UDP $port"
  & netsh.exe advfirewall firewall delete rule name="$name" | Out-Null 2>$null
  & netsh.exe advfirewall firewall add rule `
    name="$name" `
    dir=in `
    action=allow `
    protocol=UDP `
    localport=$port `
    profile=private,domain,public | Out-Null
  Write-Host "Allowed inbound UDP $port for GameCult LAN RUDP."
}
