$ErrorActionPreference = "Stop"

$serviceName = "WireGuardTunnel`$wg-gamecult-starfire"
$repoRoot = Split-Path -Parent $PSScriptRoot
$logPath = Join-Path $repoRoot "scratch\wireguard-uac-start.log"
$configPath = "C:\Meta\GameCult\WireGuard\wg-gamecult-starfire.conf"

New-Item -ItemType Directory -Force -Path (Split-Path -Parent $logPath) | Out-Null

try {
  if (-not (Test-Path -LiteralPath $configPath)) {
    throw "Missing WireGuard config at $configPath"
  }

  $config = Get-Content -Raw -LiteralPath $configPath
  $nightwingHomeResolves = $false
  try {
    $nightwingHomeResolves = [bool](Resolve-DnsName Nightwing.Home -ErrorAction Stop)
  } catch {
    $nightwingHomeResolves = $false
  }

  $configChanged = $false
  $backupPath = $null
  if (-not $nightwingHomeResolves -and $config -match "Endpoint\s*=\s*Nightwing\.Home:51820") {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $backupPath = "$configPath.bak-nightwing-home-$stamp"
    Copy-Item -LiteralPath $configPath -Destination $backupPath
    $config = $config -replace "Endpoint\s*=\s*Nightwing\.Home:51820", "Endpoint = 192.168.1.75:51820"
    Set-Content -Encoding ASCII -LiteralPath $configPath -Value $config
    $configChanged = $true
  }

  Start-Service -Name $serviceName
  Start-Sleep -Seconds 5

  $service = Get-Service -Name $serviceName
  $ipAddresses = Get-NetIPAddress |
    Where-Object { $_.IPAddress -match "^10\.77\." -or $_.IPAddress -eq "192.168.1.66" } |
    Select-Object InterfaceAlias, IPAddress, PrefixLength
  $portProxy = netsh interface portproxy show all
  $ollamaTags = & curl.exe -fsS --connect-timeout 5 --max-time 10 http://127.0.0.1:11434/api/tags

  @(
    "completedAt=$((Get-Date).ToString("o"))"
    "serviceName=$serviceName"
    "configPath=$configPath"
    "nightwingHomeResolves=$nightwingHomeResolves"
    "configChanged=$configChanged"
    "backupPath=$backupPath"
    "serviceStatus=$($service.Status)"
    "serviceStartType=$($service.StartType)"
    ""
    "ipAddresses:"
    ($ipAddresses | Format-Table -AutoSize | Out-String)
    ""
    "portProxy:"
    ($portProxy | Out-String)
    ""
    "ollamaTags:"
    $ollamaTags
  ) | Set-Content -Encoding UTF8 -LiteralPath $logPath
} catch {
  @(
    "completedAt=$((Get-Date).ToString("o"))"
    "serviceName=$serviceName"
    "error=$($_.Exception.Message)"
    "category=$($_.CategoryInfo)"
  ) | Set-Content -Encoding UTF8 -LiteralPath $logPath
  throw
}
