param(
  [string] $RavenHost = "raven.Home",
  [string] $MuninnExe = "C:\Meta\Odin\Muninn\muninn.exe",
  [string] $StorePath = "C:\Meta\Odin\state\muninn.telemetry.cc",
  [string] $ActivateStorePath = "C:\Meta\Odin\state\muninn.activate.cc",
  [string] $LogRoot = "C:\Meta\Odin\logs\muninn",
  [string] $LocalLoopbackScript = (Join-Path $PSScriptRoot "wasapi-loopback-capture.ps1"),
  [string] $LoopbackScript = "C:\Meta\Odin\Muninn\scripts\wasapi-loopback-capture.ps1",
  [string] $Ffmpeg = "C:\Users\Madman's Lullaby\AppData\Local\Microsoft\WinGet\Links\ffmpeg.exe",
  [string] $MediaTargetUri = $env:MUNINN_MEDIA_TARGET_URI,
  [string] $AudioDevice = "Realtek",
  [string[]] $VideoSources = @(),
  [string[]] $AudioSources = @(),
  [string] $IdunnRudpHealth = $env:IDUNN_RUDP_HEALTH,
  [string] $IdunnDaemon = "muninn",
  [string] $IdunnHealthContract = "muninn.cultnet-rudp-remote-telemetry-health",
  [string] $OdinCultMeshUri = $(if ($env:ODIN_CULTMESH_URI) { $env:ODIN_CULTMESH_URI } else { "cultmesh://odin/rendezvous/provider-catalog" }),
  [string[]] $MoveStates = @("xbox-raven=xinput://0"),
  [string] $HidControllerRudpTarget = "",
  [string] $HidControllerRudpBind = "0.0.0.0:17887",
  [string] $HidControllerRudpAdvertise = $env:MUNINN_HID_CONTROLLER_RUDP_ADVERTISE,
  [int] $ConnectTimeoutSeconds = 10,
  [int] $ServeStartTimeoutSeconds = 20,
  [string] $SshUser = "madman's lullaby",
  [string] $IdentityFile = "C:\Users\Meta\.ssh\id_ed25519_192_168_1_84"
)

$ErrorActionPreference = "Stop"

if ($env:IDUNN_ACTUATOR -ne "1" -or $env:IDUNN_COMMAND_AUTHORITY -ne "idunn-daemon") {
  throw "restart-muninn.ps1 is an Idunn actuator body. Redeploy by poking Idunn; direct service restart is not an owned path."
}

if (-not (Test-Path -LiteralPath $LocalLoopbackScript)) {
  throw "Local Muninn loopback script not found at $LocalLoopbackScript"
}
if ([string]::IsNullOrWhiteSpace($IdunnRudpHealth)) {
  throw "Idunn RUDP health endpoint must be supplied by -IdunnRudpHealth or IDUNN_RUDP_HEALTH; no Starfire LAN default is allowed."
}
if ([string]::IsNullOrWhiteSpace($MediaTargetUri) -or -not $MediaTargetUri.StartsWith("cultmesh://", [System.StringComparison]::OrdinalIgnoreCase)) {
  throw "Muninn media target URI must be supplied by -MediaTargetUri or MUNINN_MEDIA_TARGET_URI and must start with cultmesh://."
}
if (-not [string]::IsNullOrWhiteSpace($HidControllerRudpBind) -and [string]::IsNullOrWhiteSpace($HidControllerRudpAdvertise)) {
  throw "HID controller RUDP advertise endpoint must be supplied by -HidControllerRudpAdvertise or MUNINN_HID_CONTROLLER_RUDP_ADVERTISE when HID RUDP bind is enabled; no Raven LAN default is allowed."
}

function Set-AsciiFile {
  param(
    [Parameter(Mandatory = $true)] [string] $Path,
    [Parameter(Mandatory = $true)] [string] $Content
  )

  [System.IO.File]::WriteAllText($Path, ($Content -replace "`r?`n", "`r`n"), [System.Text.Encoding]::ASCII)
}

function ConvertTo-PowerShellStringLiteral {
  param(
    [Parameter(Mandatory = $true)] [string] $Value
  )

  return "'" + $Value.Replace("'", "''") + "'"
}

function ConvertTo-PowerShellArrayLiteral {
  param(
    [Parameter(Mandatory = $true)] [string[]] $Values
  )

  $lines = $Values | ForEach-Object { "  {0}" -f (ConvertTo-PowerShellStringLiteral $_) }
  return "@(`r`n{0}`r`n)" -f ($lines -join ",`r`n")
}

function Get-SshCommonArgs {
  $args = @(
    "-o", "BatchMode=yes",
    "-o", "ConnectTimeout=$ConnectTimeoutSeconds",
    "-o", "ConnectionAttempts=1"
  )
  if (-not [string]::IsNullOrWhiteSpace($IdentityFile)) {
    $args += @("-i", $IdentityFile)
  }
  return $args
}

function Get-SshTarget {
  param([Parameter(Mandatory = $true)] [string] $Target)

  if ([string]::IsNullOrWhiteSpace($SshUser)) {
    return $Target
  }
  return "${SshUser}@${Target}"
}

function Test-LikelyVirtualDisplayToken {
  param([Parameter(Mandatory = $true)] [string] $Token)

  $trimmed = $Token.Trim()
  if ([string]::IsNullOrWhiteSpace($trimmed)) {
    return $true
  }
  return (
    $trimmed.StartsWith('MSBDD_', [System.StringComparison]::OrdinalIgnoreCase) -or
    $trimmed.StartsWith('MSNIL', [System.StringComparison]::OrdinalIgnoreCase) -or
    $trimmed.StartsWith('NOEDID', [System.StringComparison]::OrdinalIgnoreCase) -or
    $trimmed.StartsWith('UGD', [System.StringComparison]::OrdinalIgnoreCase)
  )
}

function Get-GraphicsDriverDisplayTokens {
  $connectivityRoot = 'HKLM:\SYSTEM\CurrentControlSet\Control\GraphicsDrivers\Connectivity'
  if (-not (Test-Path -LiteralPath $connectivityRoot)) {
    return @()
  }

  $bestTokens = @()
  $bestScore = [int]::MinValue
  foreach ($key in Get-ChildItem -LiteralPath $connectivityRoot -ErrorAction SilentlyContinue) {
    $item = Get-ItemProperty -LiteralPath $key.PSPath -ErrorAction SilentlyContinue
    if ($null -eq $item) {
      continue
    }
    foreach ($property in $item.PSObject.Properties) {
      if ($property.Name -like 'PS*' -or $property.Value -isnot [string]) {
        continue
      }
      $tokens = @(
        $property.Value.Split('+') |
          ForEach-Object { $_.Trim() } |
          Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
      )
      if ($tokens.Count -lt 1) {
        continue
      }
      $realCount = @($tokens | Where-Object { -not (Test-LikelyVirtualDisplayToken $_) }).Count
      $score = ($realCount * 100) + $tokens.Count
      if ($score -gt $bestScore) {
        $bestScore = $score
        $bestTokens = $tokens
      }
    }
  }

  return @($bestTokens | Select-Object -Unique)
}

function Get-DefaultMuninnVideoSourceLabels {
  param([Parameter(Mandatory = $true)] [string] $HostLabel)

  $displayNames = @(
    Get-CimInstance Win32_DesktopMonitor -ErrorAction SilentlyContinue |
      Where-Object { [string]::IsNullOrWhiteSpace($_.Status) -or $_.Status -eq 'OK' } |
      ForEach-Object {
        if ([string]::IsNullOrWhiteSpace($_.Name)) {
          'Display'
        } else {
          $_.Name.Trim()
        }
      }
  )

  $useRegistryFallback = (
    $displayNames.Count -lt 1 -or
    ($displayNames.Count -eq 1 -and (
      $displayNames[0] -match '^\s*WinDisc\s*$' -or
      $displayNames[0] -match '^\s*Generic PnP Monitor\s*$' -or
      $displayNames[0] -match '^\s*Display\s*$'
    ))
  )

  if ($useRegistryFallback) {
    $displayTokens = @(Get-GraphicsDriverDisplayTokens)
    if ($displayTokens.Count -gt 0) {
      return @(for ($index = 0; $index -lt $displayTokens.Count; $index++) {
        '{0} display {1}' -f $HostLabel, ($index + 1)
      })
    }
  }

  if ($displayNames.Count -lt 1) {
    $displayNames = @('Display')
  }

  return @(for ($index = 0; $index -lt $displayNames.Count; $index++) {
    $monitorName = $displayNames[$index]
    if ($monitorName -eq 'Display') {
      '{0} display {1}' -f $HostLabel, ($index + 1)
    } else {
      '{0} display {1} ({2})' -f $HostLabel, ($index + 1), $monitorName
    }
  })
}

function New-HiddenNativeLauncherPowerShellContent {
  param(
    [Parameter(Mandatory = $false)] [string] $StorePath,
    [Parameter(Mandatory = $true)] [string] $LogRoot,
    [Parameter(Mandatory = $true)] [string[]] $Arguments,
    [Parameter(Mandatory = $true)] [string] $FilePath,
    [Parameter(Mandatory = $true)] [string] $StdoutPath,
    [Parameter(Mandatory = $true)] [string] $StderrPath,
    [Parameter(Mandatory = $true)] [string] $PidPath,
    [Parameter(Mandatory = $false)] [string] $WorkingDirectory,
    [Parameter(Mandatory = $false)] [string] $DynamicArgumentsScript,
    [Parameter(Mandatory = $false)] [switch] $WaitForExit
  )

  $template = @'
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
__STORE_DIR_LINE__
New-Item -ItemType Directory -Force -Path __LOG_ROOT__ | Out-Null
$launcherLog = __PID_PATH__ + '.launcher.log'
('launcher-start ' + [DateTime]::UtcNow.ToString('o')) | Set-Content -Encoding ASCII -LiteralPath $launcherLog
$arguments = __ARGUMENTS__
__DYNAMIC_ARGUMENTS_SCRIPT__
function Quote-NativeArgument([string] $Value) {
  if ($Value -match '[\s"]') {
    return '"' + $Value.Replace('"', '\"') + '"'
  }
  return $Value
}
$argumentLine = ($arguments | ForEach-Object { Quote-NativeArgument $_ }) -join ' '
('launcher-args ' + $argumentLine) | Add-Content -Encoding ASCII -LiteralPath $launcherLog
if (__RUN_INLINE__) {
  $PID | Set-Content -Encoding ASCII -LiteralPath __PID_PATH__
  ('launcher-pid ' + $PID + ' powershell-inline') | Add-Content -Encoding ASCII -LiteralPath $launcherLog
  $previousErrorActionPreference = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  try {
    & __FILE_PATH__ @arguments > __STDOUT_PATH__ 2> __STDERR_PATH__
    $exitCode = $LASTEXITCODE
  } catch {
    ('launcher-error ' + $_.Exception.Message) | Add-Content -Encoding ASCII -LiteralPath $launcherLog
    throw
  } finally {
    $ErrorActionPreference = $previousErrorActionPreference
  }
  ('launcher-exit ' + $exitCode) | Add-Content -Encoding ASCII -LiteralPath $launcherLog
  exit $exitCode
}
try {
  $process = Start-Process -FilePath __FILE_PATH__ -ArgumentList $argumentLine __WORKING_DIRECTORY_CLAUSE__-WindowStyle Hidden -PassThru -RedirectStandardOutput __STDOUT_PATH__ -RedirectStandardError __STDERR_PATH__
  $process.Id | Set-Content -Encoding ASCII -LiteralPath __PID_PATH__
  ('launcher-pid ' + $process.Id) | Add-Content -Encoding ASCII -LiteralPath $launcherLog
} catch {
  ('launcher-error ' + $_.Exception.Message) | Add-Content -Encoding ASCII -LiteralPath $launcherLog
  throw
}
__WAIT_FOR_EXIT_LINE__
'@

  $storeDirLine = if ([string]::IsNullOrWhiteSpace($StorePath)) {
    ""
  } else {
    "New-Item -ItemType Directory -Force -Path (Split-Path -Parent {0}) | Out-Null" -f (ConvertTo-PowerShellStringLiteral $StorePath)
  }
  $workingDirectoryClause = if ([string]::IsNullOrWhiteSpace($WorkingDirectory)) {
    ""
  } else {
    "-WorkingDirectory {0} " -f (ConvertTo-PowerShellStringLiteral $WorkingDirectory)
  }

  return $template.
    Replace("__STORE_DIR_LINE__", $storeDirLine).
    Replace("__LOG_ROOT__", (ConvertTo-PowerShellStringLiteral $LogRoot)).
    Replace("__ARGUMENTS__", (ConvertTo-PowerShellArrayLiteral $Arguments)).
    Replace("__DYNAMIC_ARGUMENTS_SCRIPT__", $DynamicArgumentsScript).
    Replace("__FILE_PATH__", (ConvertTo-PowerShellStringLiteral $FilePath)).
    Replace("__WORKING_DIRECTORY_CLAUSE__", $workingDirectoryClause).
    Replace("__STDOUT_PATH__", (ConvertTo-PowerShellStringLiteral $StdoutPath)).
    Replace("__STDERR_PATH__", (ConvertTo-PowerShellStringLiteral $StderrPath)).
    Replace("__PID_PATH__", (ConvertTo-PowerShellStringLiteral $PidPath)).
    Replace("__RUN_INLINE__", $(if ($WaitForExit) { '$true' } else { '$false' })).
    Replace("__WAIT_FOR_EXIT_LINE__", $(if ($WaitForExit) { '$process.WaitForExit(); exit $process.ExitCode' } else { "" }))
}

function New-HiddenPowerShellVbsLauncherContent {
  param(
    [Parameter(Mandatory = $true)] [string] $PsPath,
    [Parameter(Mandatory = $false)] [switch] $WaitForExit
  )

  return (@'
Set fso = CreateObject("Scripting.FileSystemObject")
scriptDir = fso.GetParentFolderName(WScript.ScriptFullName)
psLauncher = "{0}"
logPath = psLauncher & ".vbs.log"
Set logFile = fso.OpenTextFile(logPath, 8, True)
logFile.WriteLine "vbs-start " & Now
logFile.Close
Set shell = CreateObject("WScript.Shell")
shell.CurrentDirectory = scriptDir
exitCode = shell.Run("powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File """ & psLauncher & """", 0, {1})
Set logFile = fso.OpenTextFile(logPath, 8, True)
logFile.WriteLine "vbs-exit " & exitCode & " " & Now
logFile.Close
'@) -f $PsPath, ($(if ($WaitForExit) { "True" } else { "False" }))
}

function New-WscriptCmdLauncherContent {
  param(
    [Parameter(Mandatory = $true)] [string] $WorkingDirectory,
    [Parameter(Mandatory = $true)] [string] $VbsPath
  )

  return (@'
@echo off
cd /d "{0}"
wscript.exe //B //Nologo "{1}"
'@) -f $WorkingDirectory, $VbsPath
}

function Invoke-RavenUploadedPowerShell {
  param(
    [Parameter(Mandatory = $true)] [string] $RavenHost,
    [Parameter(Mandatory = $true)] [string] $RemoteScriptContent,
    [Parameter(Mandatory = $true)] [object[]] $UploadSpecs,
    [Parameter(Mandatory = $true)] [string] $TempPrefix
  )

  $uploadId = [guid]::NewGuid().ToString("N")
  $localTempRoot = Join-Path $env:TEMP "$TempPrefix-$uploadId"
  $localRemoteScript = Join-Path $localTempRoot "$TempPrefix-$uploadId.ps1"
  $localSftpBatch = Join-Path $localTempRoot "$TempPrefix-$uploadId.sftp"
  $remoteSftpPath = "C:/Windows/Temp/$TempPrefix-$uploadId.ps1"
  $remotePsPath = "C:\Windows\Temp\$TempPrefix-$uploadId.ps1"

  try {
    New-Item -ItemType Directory -Force -Path $localTempRoot | Out-Null
    Set-AsciiFile -Path $localRemoteScript -Content $RemoteScriptContent

    $batchLines = @()
    foreach ($spec in $UploadSpecs) {
      $batchLines += 'put "{0}" "{1}"' -f $spec.LocalPath, $spec.RemotePath
    }
    $batchLines += 'put "{0}" "{1}"' -f $localRemoteScript, $remoteSftpPath
    Set-AsciiFile -Path $localSftpBatch -Content ($batchLines -join "`r`n")

    $commonArgs = Get-SshCommonArgs
    $sshTarget = Get-SshTarget -Target $RavenHost
    & sftp.exe @commonArgs -b $localSftpBatch $sshTarget
    if ($LASTEXITCODE -ne 0) {
      exit $LASTEXITCODE
    }

    $remoteRunner = @"
`$ErrorActionPreference = "Stop"
`$ProgressPreference = "SilentlyContinue"
try {
  & "$remotePsPath"
  exit 0
} finally {
  Remove-Item -LiteralPath "$remotePsPath" -Force -ErrorAction SilentlyContinue
}
"@
    $encodedRunner = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($remoteRunner))
    $commonArgs = Get-SshCommonArgs
    $sshTarget = Get-SshTarget -Target $RavenHost
    & ssh.exe @commonArgs $sshTarget "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -OutputFormat Text -EncodedCommand $encodedRunner"
  } finally {
    Remove-Item -LiteralPath $localTempRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}

$muninnDir = Split-Path -Parent $MuninnExe
$serveCmdPath = Join-Path $muninnDir "start-muninn-serve.cmd"
$servePsPath = Join-Path $muninnDir "start-muninn-serve.ps1"
$serveVbsPath = Join-Path $muninnDir "start-muninn-serve-hidden.vbs"

$serveArguments = @(
  "serve",
  "--store", $StorePath,
  "--activate-store", $ActivateStorePath,
  "--log-root", $LogRoot,
  "--host", "raven",
  "--stream", "muninn.raven.av.rudp",
  "--target-host", $MediaTargetUri,
  "--media-transport", "rudp"
)
if (-not [string]::IsNullOrWhiteSpace($OdinCultMeshUri)) {
  $serveArguments += @("--odin-cultmesh-uri", $OdinCultMeshUri)
}
if (-not [string]::IsNullOrWhiteSpace($HidControllerRudpTarget)) {
  $serveArguments += @("--hid-controller-rudp-target", $HidControllerRudpTarget)
}
if (-not [string]::IsNullOrWhiteSpace($HidControllerRudpBind)) {
  $serveArguments += @("--hid-controller-rudp-bind", $HidControllerRudpBind)
}
if (-not [string]::IsNullOrWhiteSpace($HidControllerRudpAdvertise)) {
  $serveArguments += @("--hid-controller-rudp-advertise", $HidControllerRudpAdvertise)
}
foreach ($videoSource in $VideoSources) {
  $serveArguments += @("--video-source", $videoSource)
}
foreach ($audioSource in $AudioSources) {
  $serveArguments += @("--audio-source", $audioSource)
}
foreach ($moveState in $MoveStates) {
  if (-not [string]::IsNullOrWhiteSpace($moveState)) {
    $serveArguments += @("--move-state", $moveState)
  }
}
$serveArguments += @(
  "--audio-device", $AudioDevice,
  "--ffmpeg", $Ffmpeg,
  "--loopback-script", $LoopbackScript,
  "--interval-seconds", "15",
  "--idunn-rudp-health", $IdunnRudpHealth,
  "--idunn-daemon", $IdunnDaemon,
  "--idunn-health-contract", $IdunnHealthContract
)
$serveDynamicArgumentsScript = @'
function Test-LikelyVirtualDisplayToken {
  param([Parameter(Mandatory = $true)] [string] $Token)

  $trimmed = $Token.Trim()
  if ([string]::IsNullOrWhiteSpace($trimmed)) {
    return $true
  }
  return (
    $trimmed.StartsWith('MSBDD_', [System.StringComparison]::OrdinalIgnoreCase) -or
    $trimmed.StartsWith('MSNIL', [System.StringComparison]::OrdinalIgnoreCase) -or
    $trimmed.StartsWith('NOEDID', [System.StringComparison]::OrdinalIgnoreCase) -or
    $trimmed.StartsWith('UGD', [System.StringComparison]::OrdinalIgnoreCase)
  )
}

function Get-GraphicsDriverDisplayTokens {
  $connectivityRoot = 'HKLM:\SYSTEM\CurrentControlSet\Control\GraphicsDrivers\Connectivity'
  if (-not (Test-Path -LiteralPath $connectivityRoot)) {
    return @()
  }

  $bestTokens = @()
  $bestScore = [int]::MinValue
  foreach ($key in Get-ChildItem -LiteralPath $connectivityRoot -ErrorAction SilentlyContinue) {
    $item = Get-ItemProperty -LiteralPath $key.PSPath -ErrorAction SilentlyContinue
    if ($null -eq $item) {
      continue
    }
    foreach ($property in $item.PSObject.Properties) {
      if ($property.Name -like 'PS*' -or $property.Value -isnot [string]) {
        continue
      }
      $tokens = @(
        $property.Value.Split('+') |
          ForEach-Object { $_.Trim() } |
          Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
      )
      if ($tokens.Count -lt 1) {
        continue
      }
      $realCount = @($tokens | Where-Object { -not (Test-LikelyVirtualDisplayToken $_) }).Count
      $score = ($realCount * 100) + $tokens.Count
      if ($score -gt $bestScore) {
        $bestScore = $score
        $bestTokens = $tokens
      }
    }
  }

  return @($bestTokens | Select-Object -Unique)
}

function Get-DefaultMuninnVideoSourceLabels {
  param([Parameter(Mandatory = $true)] [string] $HostLabel)

  $displayNames = @(
    Get-CimInstance Win32_DesktopMonitor -ErrorAction SilentlyContinue |
      Where-Object { [string]::IsNullOrWhiteSpace($_.Status) -or $_.Status -eq 'OK' } |
      ForEach-Object {
        if ([string]::IsNullOrWhiteSpace($_.Name)) {
          'Display'
        } else {
          $_.Name.Trim()
        }
      }
  )

  $useRegistryFallback = (
    $displayNames.Count -lt 1 -or
    ($displayNames.Count -eq 1 -and (
      $displayNames[0] -match '^\s*WinDisc\s*$' -or
      $displayNames[0] -match '^\s*Generic PnP Monitor\s*$' -or
      $displayNames[0] -match '^\s*Display\s*$'
    ))
  )

  if ($useRegistryFallback) {
    $displayTokens = @(Get-GraphicsDriverDisplayTokens)
    if ($displayTokens.Count -gt 0) {
      return @(for ($index = 0; $index -lt $displayTokens.Count; $index++) {
        '{0} display {1}' -f $HostLabel, ($index + 1)
      })
    }
  }

  if ($displayNames.Count -lt 1) {
    $displayNames = @('Display')
  }

  return @(for ($index = 0; $index -lt $displayNames.Count; $index++) {
    $monitorName = $displayNames[$index]
    if ($monitorName -eq 'Display') {
      '{0} display {1}' -f $HostLabel, ($index + 1)
    } else {
      '{0} display {1} ({2})' -f $HostLabel, ($index + 1), $monitorName
    }
  })
}

$hostArgumentIndex = [Array]::IndexOf($arguments, '--host')
$hostLabel = if ($hostArgumentIndex -ge 0 -and ($hostArgumentIndex + 1) -lt $arguments.Count) {
  $arguments[$hostArgumentIndex + 1]
} else {
  'host'
}
$hasExplicitVideoSources = @($arguments | Where-Object { $_ -eq '--video-source' }).Count -gt 0
$hasExplicitAudioSources = @($arguments | Where-Object { $_ -eq '--audio-source' }).Count -gt 0
if (-not ('Muninn.AudioEndpointDiscovery' -as [type])) {
  Add-Type -TypeDefinition @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

namespace Muninn {
    public static class AudioEndpointDiscovery {
        const int eRender = 0;
        const int eCapture = 1;
        const uint DEVICE_STATE_ACTIVE = 0x00000001;
        static readonly PROPERTYKEY DeviceFriendlyNameKey = new PROPERTYKEY(
            new Guid("A45C254E-DF1C-4EFD-8020-67D146A850E0"),
            14);

        [StructLayout(LayoutKind.Sequential)]
        struct PROPERTYKEY {
            public Guid fmtid;
            public uint pid;

            public PROPERTYKEY(Guid formatId, uint propertyId) {
                fmtid = formatId;
                pid = propertyId;
            }
        }

        [StructLayout(LayoutKind.Sequential)]
        struct PROPVARIANT {
            public ushort vt;
            public ushort wReserved1;
            public ushort wReserved2;
            public ushort wReserved3;
            public IntPtr pointerValue;
            public int intValue;
        }

        [ComImport]
        [Guid("BCDE0395-E52F-467C-8E3D-C4579291692E")]
        class MMDeviceEnumerator { }

        [ComImport]
        [Guid("A95664D2-9614-4F35-A746-DE8DB63617E6")]
        [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        interface IMMDeviceEnumerator {
            [PreserveSig]
            int EnumAudioEndpoints(int dataFlow, uint stateMask, out IMMDeviceCollection devices);
        }

        [ComImport]
        [Guid("0BD7A1BE-7A1A-44DB-8397-CC5392387B5E")]
        [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        interface IMMDeviceCollection {
            [PreserveSig]
            int GetCount(out uint count);
            [PreserveSig]
            int Item(uint index, out IMMDevice device);
        }

        [ComImport]
        [Guid("D666063F-1587-4E43-81F1-B948E807363F")]
        [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        interface IMMDevice {
            [PreserveSig]
            int Activate(ref Guid iid, int dwClsCtx, IntPtr pActivationParams, out object audioClient);
            [PreserveSig]
            int OpenPropertyStore(uint access, out IPropertyStore properties);
        }

        [ComImport]
        [Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99")]
        [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        interface IPropertyStore {
            [PreserveSig]
            int GetCount(out uint propertyCount);
            [PreserveSig]
            int GetAt(uint propertyIndex, out PROPERTYKEY key);
            [PreserveSig]
            int GetValue(ref PROPERTYKEY key, out PROPVARIANT value);
            [PreserveSig]
            int SetValue(ref PROPERTYKEY key, ref PROPVARIANT value);
            [PreserveSig]
            int Commit();
        }

        [DllImport("ole32.dll")]
        static extern int PropVariantClear(ref PROPVARIANT variant);

        public static string[] FriendlyNames(int dataFlow) {
            var names = new List<string>();
            IMMDeviceEnumerator enumerator = (IMMDeviceEnumerator)(new MMDeviceEnumerator());
            IMMDeviceCollection devices;
            Marshal.ThrowExceptionForHR(enumerator.EnumAudioEndpoints(dataFlow, DEVICE_STATE_ACTIVE, out devices));
            try {
                uint count;
                Marshal.ThrowExceptionForHR(devices.GetCount(out count));
                for (uint index = 0; index < count; index++) {
                    IMMDevice device;
                    Marshal.ThrowExceptionForHR(devices.Item(index, out device));
                    try {
                        var name = DeviceFriendlyName(device);
                        if (!String.IsNullOrWhiteSpace(name)) {
                            names.Add(name.Trim());
                        }
                    } finally {
                        Marshal.ReleaseComObject(device);
                    }
                }
            } finally {
                Marshal.ReleaseComObject(devices);
                Marshal.ReleaseComObject(enumerator);
            }
            return names.ToArray();
        }

        static string DeviceFriendlyName(IMMDevice device) {
            IPropertyStore store;
            Marshal.ThrowExceptionForHR(device.OpenPropertyStore(0, out store));
            try {
                PROPERTYKEY key = DeviceFriendlyNameKey;
                PROPVARIANT value;
                Marshal.ThrowExceptionForHR(store.GetValue(ref key, out value));
                try {
                    if (value.vt == 31 && value.pointerValue != IntPtr.Zero) {
                        return Marshal.PtrToStringUni(value.pointerValue) ?? "";
                    }
                    return "";
                } finally {
                    PropVariantClear(ref value);
                }
            } finally {
                Marshal.ReleaseComObject(store);
            }
        }
    }
}
"@
}
if (-not $hasExplicitVideoSources) {
  $videoSourceLabels = @(Get-DefaultMuninnVideoSourceLabels -HostLabel $hostLabel)
  for ($index = 0; $index -lt $videoSourceLabels.Count; $index++) {
    $arguments += @('--video-source', ('display:{0}={1}' -f $index, $videoSourceLabels[$index]))
  }
}
if (-not $hasExplicitAudioSources) {
  $loopbackNames = @([Muninn.AudioEndpointDiscovery]::FriendlyNames(0) | Sort-Object -Unique)
  $inputNames = @([Muninn.AudioEndpointDiscovery]::FriendlyNames(1) | Sort-Object -Unique)
  if ($loopbackNames.Count -lt 1 -and $inputNames.Count -lt 1) {
    $audioDeviceIndex = [Array]::IndexOf($arguments, '--audio-device')
    if ($audioDeviceIndex -ge 0 -and ($audioDeviceIndex + 1) -lt $arguments.Count) {
      $loopbackNames = @($arguments[$audioDeviceIndex + 1])
    }
  }
  foreach ($audioName in $loopbackNames) {
    $arguments += @('--audio-source', ('wasapi-loopback:{0}={1} loopback ({0})' -f $audioName.Trim(), $hostLabel))
  }
  foreach ($audioName in $inputNames) {
    $arguments += @('--audio-source', ('wasapi-input:{0}={1} input ({0})' -f $audioName.Trim(), $hostLabel))
  }
}
'@

$servePsContent = New-HiddenNativeLauncherPowerShellContent `
  -StorePath $StorePath `
  -LogRoot $LogRoot `
  -Arguments $serveArguments `
  -FilePath $MuninnExe `
  -StdoutPath (Join-Path $LogRoot "muninn-serve.out.log") `
  -StderrPath (Join-Path $LogRoot "muninn-serve.err.log") `
  -PidPath (Join-Path $LogRoot "muninn-serve.pid") `
  -DynamicArgumentsScript $serveDynamicArgumentsScript

$uploadRoot = Join-Path $env:TEMP ("odin-raven-muninn-launchers-" + [guid]::NewGuid().ToString("N"))

try {
  New-Item -ItemType Directory -Force -Path $uploadRoot | Out-Null

  $localServePs = Join-Path $uploadRoot "start-muninn-serve.ps1"
  $localServeVbs = Join-Path $uploadRoot "start-muninn-serve-hidden.vbs"
  $localServeCmd = Join-Path $uploadRoot "start-muninn-serve.cmd"

  Set-AsciiFile -Path $localServePs -Content $servePsContent
  Set-AsciiFile -Path $localServeVbs -Content (New-HiddenPowerShellVbsLauncherContent -PsPath $servePsPath)
  Set-AsciiFile -Path $localServeCmd -Content (New-WscriptCmdLauncherContent -WorkingDirectory $muninnDir -VbsPath $serveVbsPath)

  $remoteScript = @"
`$ErrorActionPreference = "Stop"
`$ProgressPreference = "SilentlyContinue"

if (-not (Test-Path -LiteralPath "$MuninnExe")) {
  throw "Muninn executable not found at $MuninnExe"
}
if (-not (Test-Path -LiteralPath "$LoopbackScript")) {
  throw "Muninn loopback script not found at $LoopbackScript"
}
if (-not (Test-Path -LiteralPath "$Ffmpeg")) {
  throw "FFmpeg executable not found at $Ffmpeg"
}
foreach (`$path in @(
  "$servePsPath",
  "$serveVbsPath"
)) {
  if (-not (Test-Path -LiteralPath `$path)) {
    throw "Required launcher path not found at `$path"
  }
}

`$obsoleteActivateTask = Get-ScheduledTask -TaskName "GameCult-Muninn-Activate" -ErrorAction SilentlyContinue
if (`$null -ne `$obsoleteActivateTask) {
  Unregister-ScheduledTask -TaskName "GameCult-Muninn-Activate" -Confirm:`$false | Out-Null
}
`$obsoleteVideoProofTask = Get-ScheduledTask -TaskName "GameCult-Muninn-VideoProof" -ErrorAction SilentlyContinue
if (`$null -ne `$obsoleteVideoProofTask) {
  Unregister-ScheduledTask -TaskName "GameCult-Muninn-VideoProof" -Confirm:`$false | Out-Null
}

Get-CimInstance Win32_Process |
  Where-Object { `$_.Name -like "muninn*.exe" } |
  ForEach-Object {
    & cmd.exe /c ("taskkill /PID {0} /T /F >nul 2>&1" -f `$_.ProcessId) | Out-Null
  }

New-Item -ItemType Directory -Force -Path "$LogRoot" | Out-Null
New-Item -ItemType Directory -Force -Path (Split-Path -Parent "$StorePath") | Out-Null
& netsh.exe advfirewall firewall delete rule name="GameCult Muninn Capture Command RUDP" | Out-Null 2>`$null
if (-not [string]::IsNullOrWhiteSpace("$HidControllerRudpBind")) {
  `$hidControllerUdpPort = ([string] "$HidControllerRudpBind").Split(':')[-1]
  & netsh.exe advfirewall firewall delete rule name="GameCult Muninn HID Controller RUDP" | Out-Null 2>`$null
  & netsh.exe advfirewall firewall add rule name="GameCult Muninn HID Controller RUDP" dir=in action=allow protocol=UDP localport=`$hidControllerUdpPort | Out-Null
}

function Register-HiddenVbsTask {
  param(
    [Parameter(Mandatory = `$true)] [string] `$TaskName,
    [Parameter(Mandatory = `$true)] [string] `$VbsPath
  )

  `$taskAction = New-ScheduledTaskAction -Execute "$env:SystemRoot\System32\wscript.exe" -Argument "//B //Nologo ""`$VbsPath""" -WorkingDirectory (Split-Path -Parent `$VbsPath)
  `$taskTrigger = New-ScheduledTaskTrigger -Once -At ([DateTime]::Today.AddHours(23).AddMinutes(59))
  `$taskPrincipal = New-ScheduledTaskPrincipal -UserId ([System.Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Highest
  `$taskSettings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew
  Register-ScheduledTask -TaskName `$TaskName -Action `$taskAction -Trigger `$taskTrigger -Principal `$taskPrincipal -Settings `$taskSettings -Force | Out-Null
}

function Register-HiddenPowerShellTask {
  param(
    [Parameter(Mandatory = `$true)] [string] `$TaskName,
    [Parameter(Mandatory = `$true)] [string] `$PsPath
  )

  `$taskArguments = '-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File "' + `$PsPath + '"'
  `$taskAction = New-ScheduledTaskAction -Execute "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -Argument `$taskArguments -WorkingDirectory (Split-Path -Parent `$PsPath)
  `$taskTrigger = New-ScheduledTaskTrigger -Once -At ([DateTime]::Today.AddHours(23).AddMinutes(59))
  `$taskPrincipal = New-ScheduledTaskPrincipal -UserId ([System.Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Highest
  `$taskSettings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew
  Register-ScheduledTask -TaskName `$TaskName -Action `$taskAction -Trigger `$taskTrigger -Principal `$taskPrincipal -Settings `$taskSettings -Force | Out-Null
}

function Assert-HiddenVbsTask {
  param(
    [Parameter(Mandatory = `$true)] [string] `$TaskName,
    [Parameter(Mandatory = `$true)] [string] `$VbsPath,
    [Parameter(Mandatory = `$true)] [string] `$PsPath
  )

  `$task = Get-ScheduledTask -TaskName `$TaskName -ErrorAction Stop
  `$action = @(`$task.Actions)[0]
  if (`$action.Execute -notmatch '(^|\\)wscript\.exe$') {
    throw "`$TaskName action executes `$(`$action.Execute), expected wscript.exe"
  }
  if (`$action.Arguments -notlike "*`$VbsPath*") {
    throw "`$TaskName action arguments `$(`$action.Arguments) do not reference `$VbsPath"
  }
  if (`$action.Arguments -notlike "*//B*" -or `$action.Arguments -notlike "*//Nologo*") {
    throw "`$TaskName action arguments `$(`$action.Arguments) do not force background WScript execution"
  }
  if (-not (Test-Path -LiteralPath `$PsPath)) {
    throw "`$TaskName PowerShell launcher not found at `$PsPath"
  }
  `$vbs = Get-Content -LiteralPath `$VbsPath -Raw
  if (`$vbs -match 'cmdPath\s*=') {
    throw "`$TaskName hidden launcher at `$VbsPath still routes through a cmdPath trampoline"
  }
  if (`$vbs -notmatch '\.ps1') {
    throw "`$TaskName hidden launcher at `$VbsPath does not reference a PowerShell launcher"
  }
}

function Assert-HiddenPowerShellTask {
  param(
    [Parameter(Mandatory = `$true)] [string] `$TaskName,
    [Parameter(Mandatory = `$true)] [string] `$PsPath
  )

  `$task = Get-ScheduledTask -TaskName `$TaskName -ErrorAction Stop
  `$action = @(`$task.Actions)[0]
  if (`$action.Execute -notmatch 'powershell\.exe$') {
    throw "`$TaskName action executes `$(`$action.Execute), expected powershell.exe"
  }
  if (`$action.Arguments -notlike "*`$PsPath*") {
    throw "`$TaskName action arguments `$(`$action.Arguments) do not reference `$PsPath"
  }
  if (`$action.Arguments -notlike "*-File*") {
    throw "`$TaskName action arguments `$(`$action.Arguments) do not execute a PowerShell launcher"
  }
  if (-not (Test-Path -LiteralPath `$PsPath)) {
    throw "`$TaskName PowerShell launcher not found at `$PsPath"
  }
}

Register-HiddenPowerShellTask -TaskName "GameCult-Muninn" -PsPath "$servePsPath"

Assert-HiddenPowerShellTask -TaskName "GameCult-Muninn" -PsPath "$servePsPath"

Start-ScheduledTask -TaskName "GameCult-Muninn"

`$deadline = [DateTime]::UtcNow.AddSeconds($ServeStartTimeoutSeconds)
`$process = `$null
do {
  Start-Sleep -Milliseconds 500
  `$process = Get-CimInstance Win32_Process |
    Where-Object { `$_.Name -ieq "muninn.exe" -and `$_.CommandLine -like "*serve*" -and `$_.CommandLine -like "*--host*raven*" } |
    Select-Object -First 1
} while (`$null -eq `$process -and [DateTime]::UtcNow -lt `$deadline)
if (`$null -eq `$process) {
  throw "Muninn serve process did not start on Raven within $ServeStartTimeoutSeconds seconds"
}
foreach (`$pattern in @(
  "--idunn-rudp-health",
  "$IdunnRudpHealth",
  "--idunn-daemon",
  "$IdunnDaemon",
  "--idunn-health-contract",
  "$IdunnHealthContract"
)) {
  if (`$process.CommandLine -notlike "*`$pattern*") {
    throw "Muninn Raven serve command line is missing `${pattern}: `$(`$process.CommandLine)"
  }
}
"@

  $uploadSpecs = @(
    @{ LocalPath = $LocalLoopbackScript; RemotePath = ($LoopbackScript -replace "\\", "/") },
    @{ LocalPath = $localServePs; RemotePath = ($servePsPath -replace "\\", "/") },
    @{ LocalPath = $localServeVbs; RemotePath = ($serveVbsPath -replace "\\", "/") },
    @{ LocalPath = $localServeCmd; RemotePath = ($serveCmdPath -replace "\\", "/") }
  )

  Invoke-RavenUploadedPowerShell -RavenHost $RavenHost -RemoteScriptContent $remoteScript -UploadSpecs $uploadSpecs -TempPrefix "odin-raven-muninn-restart"
} finally {
  Remove-Item -LiteralPath $uploadRoot -Recurse -Force -ErrorAction SilentlyContinue
}

exit $LASTEXITCODE
