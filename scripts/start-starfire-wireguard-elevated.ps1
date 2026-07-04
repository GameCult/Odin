param(
  [string] $NightwingEndpointFallback = $env:STARFIRE_WIREGUARD_NIGHTWING_ENDPOINT_FALLBACK,
  [string] $StarfireLanAddress = $env:STARFIRE_LAN_ADDRESS
)

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
    if ([string]::IsNullOrWhiteSpace($NightwingEndpointFallback)) {
      throw "Nightwing.Home does not resolve; supply -NightwingEndpointFallback or STARFIRE_WIREGUARD_NIGHTWING_ENDPOINT_FALLBACK instead of using a baked LAN endpoint."
    }
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $backupPath = "$configPath.bak-nightwing-home-$stamp"
    Copy-Item -LiteralPath $configPath -Destination $backupPath
    $config = $config -replace "Endpoint\s*=\s*Nightwing\.Home:51820", "Endpoint = $NightwingEndpointFallback"
    Set-Content -Encoding ASCII -LiteralPath $configPath -Value $config
    $configChanged = $true
  }

  Start-Service -Name $serviceName
  Start-Sleep -Seconds 5

  $service = Get-Service -Name $serviceName
  $ipAddresses = Get-NetIPAddress |
    Where-Object { $_.IPAddress -match "^10\.77\." -or ((-not [string]::IsNullOrWhiteSpace($StarfireLanAddress)) -and $_.IPAddress -eq $StarfireLanAddress) } |
    Select-Object InterfaceAlias, IPAddress, PrefixLength
  $portProxy = netsh interface portproxy show all

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
