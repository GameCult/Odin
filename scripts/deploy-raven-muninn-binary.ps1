param(
  [string] $RavenHost = "raven",
  [string] $MuninnExe = "C:\Meta\Odin\Muninn\muninn.exe",
  [string] $LocalMuninnExe = "",
  [switch] $SkipBuild,
  [switch] $SkipRestart
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot

if ([string]::IsNullOrWhiteSpace($LocalMuninnExe)) {
  $LocalMuninnExe = Join-Path $repoRoot "target\release\muninn.exe"
}

if (-not $SkipBuild) {
  Push-Location $repoRoot
  try {
    cargo build -p muninn-daemon --release
    if ($LASTEXITCODE -ne 0) {
      throw "cargo build -p muninn-daemon --release failed with exit code $LASTEXITCODE"
    }
  } finally {
    Pop-Location
  }
}

if (-not (Test-Path -LiteralPath $LocalMuninnExe)) {
  throw "Local Muninn executable was not found at '$LocalMuninnExe'."
}

function Set-AsciiFile {
  param(
    [Parameter(Mandatory = $true)] [string] $Path,
    [Parameter(Mandatory = $true)] [string] $Content
  )

  [System.IO.File]::WriteAllText($Path, ($Content -replace "`r?`n", "`r`n"), [System.Text.Encoding]::ASCII)
}

function ConvertTo-PowerShellStringLiteral {
  param([Parameter(Mandatory = $true)] [string] $Value)
  return "'" + $Value.Replace("'", "''") + "'"
}

$deployId = [guid]::NewGuid().ToString("N")
$localTempRoot = Join-Path $env:TEMP "odin-raven-muninn-binary-$deployId"
$localRemoteScript = Join-Path $localTempRoot "deploy-raven-muninn-binary.ps1"
$localSftpBatch = Join-Path $localTempRoot "deploy-raven-muninn-binary.sftp"
$remoteExeSftpPath = "C:/Windows/Temp/muninn-$deployId.exe"
$remoteScriptSftpPath = "C:/Windows/Temp/deploy-raven-muninn-binary-$deployId.ps1"
$remoteExePath = "C:\Windows\Temp\muninn-$deployId.exe"
$remoteScriptPath = "C:\Windows\Temp\deploy-raven-muninn-binary-$deployId.ps1"
$remoteBackupPath = "$MuninnExe.bak-$deployId"

try {
  New-Item -ItemType Directory -Force -Path $localTempRoot | Out-Null

  $remoteScript = @"
`$ErrorActionPreference = "Stop"
`$ProgressPreference = "SilentlyContinue"
`$target = $(ConvertTo-PowerShellStringLiteral $MuninnExe)
`$incoming = $(ConvertTo-PowerShellStringLiteral $remoteExePath)
`$backup = $(ConvertTo-PowerShellStringLiteral $remoteBackupPath)

if (-not (Test-Path -LiteralPath `$incoming)) {
  throw "Uploaded Muninn executable was not found at `$incoming"
}

New-Item -ItemType Directory -Force -Path (Split-Path -Parent `$target) | Out-Null

Get-CimInstance Win32_Process |
  Where-Object { `$_.Name -ieq "muninn.exe" } |
  ForEach-Object {
    & taskkill.exe /PID `$_.ProcessId /T /F | Out-Null
  }

if (Test-Path -LiteralPath `$target) {
  Copy-Item -LiteralPath `$target -Destination `$backup -Force
}

Move-Item -LiteralPath `$incoming -Destination `$target -Force
`$item = Get-Item -LiteralPath `$target
Write-Output ("muninn.exe deployed length={0} path={1}" -f `$item.Length, `$item.FullName)
"@

  Set-AsciiFile -Path $localRemoteScript -Content $remoteScript
  Set-AsciiFile -Path $localSftpBatch -Content (@(
    'put "{0}" "{1}"' -f $LocalMuninnExe, $remoteExeSftpPath
    'put "{0}" "{1}"' -f $localRemoteScript, $remoteScriptSftpPath
  ) -join "`r`n")

  & sftp.exe -b $localSftpBatch $RavenHost
  if ($LASTEXITCODE -ne 0) {
    throw "sftp upload to Raven failed with exit code $LASTEXITCODE"
  }

  $remoteRunner = @"
`$ErrorActionPreference = "Stop"
try {
  & "$remoteScriptPath"
  `$code = `$LASTEXITCODE
} finally {
  Remove-Item -LiteralPath "$remoteScriptPath" -Force -ErrorAction SilentlyContinue
}
exit `$code
"@
  $encodedRunner = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($remoteRunner))
  & ssh.exe -o BatchMode=yes -o ConnectTimeout=10 $RavenHost "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand $encodedRunner"
  if ($LASTEXITCODE -ne 0) {
    throw "remote Muninn binary deploy failed with exit code $LASTEXITCODE"
  }
} finally {
  Remove-Item -LiteralPath $localTempRoot -Recurse -Force -ErrorAction SilentlyContinue
}

if (-not $SkipRestart) {
  & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "restart-muninn.ps1") `
    -RavenHost $RavenHost `
    -MuninnExe $MuninnExe
  if ($LASTEXITCODE -ne 0) {
    throw "restart-muninn.ps1 failed with exit code $LASTEXITCODE"
  }
}
