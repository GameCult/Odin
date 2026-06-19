param(
  [string] $RavenHost = "raven",
  [string] $MuninnExe = "C:\Meta\Odin\Muninn\muninn.exe",
  [string] $StorePath = "C:\Meta\Odin\state\muninn.telemetry.cc",
  [string] $ActivateStorePath = "C:\Meta\Odin\state\muninn.activate.cc",
  [string] $LogRoot = "C:\Meta\Odin\logs\muninn",
  [string] $LoopbackScript = "C:\Meta\Odin\Muninn\scripts\wasapi-loopback-capture.ps1",
  [string] $Ffmpeg = "C:\Users\Madman's Lullaby\AppData\Local\Microsoft\WinGet\Links\ffmpeg.exe",
  [string] $TargetHost = "192.168.1.66",
  [int] $Port = 5204,
  [string] $MediaTransport = "rudp",
  [string] $ObsTargetHost = "192.168.1.66",
  [int] $ObsPort = 5204,
  [string] $AudioDevice = "Realtek",
  [string[]] $VideoSources = @(),
  [string[]] $AudioSources = @(),
  [string] $IdunnRudpHealth = "192.168.1.66:17870",
  [string] $IdunnDaemon = "muninn",
  [string] $IdunnHealthContract = "muninn.cultnet-rudp-remote-telemetry-health",
  [string] $CaptureCommandRudpBind = "0.0.0.0:17873",
  [string] $CaptureCommandRudpTarget = "127.0.0.1:17873",
  [string] $ObsCatalogRudpTarget = "192.168.1.66:17874",
  [int] $ConnectTimeoutSeconds = 10,
  [int] $ServeStartTimeoutSeconds = 20,
  [string] $SshUser = "",
  [string] $IdentityFile = ""
)

$ErrorActionPreference = "Stop"

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
$activateCmdPath = Join-Path $muninnDir "activate-raven-av-srt.cmd"
$activatePsPath = Join-Path $muninnDir "activate-raven-av-srt.ps1"
$activateVbsPath = Join-Path $muninnDir "activate-raven-av-srt-hidden.vbs"
$videoProofCmdPath = Join-Path $muninnDir "muninn-raven-video-to-starfire-obs.cmd"
$videoProofPsPath = Join-Path $muninnDir "muninn-raven-video-to-starfire-obs.ps1"
$videoProofVbsPath = Join-Path $muninnDir "muninn-raven-video-to-starfire-obs-hidden.vbs"

$serveArguments = @(
  "serve",
  "--store", $StorePath,
  "--activate-store", $ActivateStorePath,
  "--log-root", $LogRoot,
  "--host", "raven",
  "--stream", "muninn.raven.av.rudp",
  "--target-host", $TargetHost,
  "--port", $Port.ToString(),
  "--media-transport", $MediaTransport
)
foreach ($videoSource in $VideoSources) {
  $serveArguments += @("--video-source", $videoSource)
}
foreach ($audioSource in $AudioSources) {
  $serveArguments += @("--audio-source", $audioSource)
}
$serveArguments += @(
  "--audio-device", $AudioDevice,
  "--ffmpeg", $Ffmpeg,
  "--loopback-script", $LoopbackScript,
  "--interval-seconds", "15",
  "--capture-command-rudp-bind", $CaptureCommandRudpBind,
  "--obs-catalog-rudp-target", $ObsCatalogRudpTarget,
  "--idunn-rudp-health", $IdunnRudpHealth,
  "--idunn-daemon", $IdunnDaemon,
  "--idunn-health-contract", $IdunnHealthContract
)

$serveDynamicArgumentsScript = @'
$hostArgumentIndex = [Array]::IndexOf($arguments, '--host')
$hostLabel = if ($hostArgumentIndex -ge 0 -and ($hostArgumentIndex + 1) -lt $arguments.Count) {
  $arguments[$hostArgumentIndex + 1]
} else {
  'host'
}
$hasExplicitVideoSources = @($arguments | Where-Object { $_ -eq '--video-source' }).Count -gt 0
$hasExplicitAudioSources = @($arguments | Where-Object { $_ -eq '--audio-source' }).Count -gt 0
if (-not ('Muninn.RenderEndpointDiscovery' -as [type])) {
  Add-Type -TypeDefinition @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

namespace Muninn {
    public static class RenderEndpointDiscovery {
        const int eRender = 0;
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

        public static string[] FriendlyNames() {
            var names = new List<string>();
            IMMDeviceEnumerator enumerator = (IMMDeviceEnumerator)(new MMDeviceEnumerator());
            IMMDeviceCollection devices;
            Marshal.ThrowExceptionForHR(enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE, out devices));
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
  if ($displayNames.Count -lt 1) {
    $displayNames = @('Display')
  }
  for ($index = 0; $index -lt $displayNames.Count; $index++) {
    $monitorName = $displayNames[$index]
    $label = if ($monitorName -eq 'Display') {
      '{0} display {1}' -f $hostLabel, ($index + 1)
    } else {
      '{0} display {1} ({2})' -f $hostLabel, ($index + 1), $monitorName
    }
    $arguments += @('--video-source', ('display:{0}={1}' -f $index, $label))
  }
}
if (-not $hasExplicitAudioSources) {
  $audioNames = @([Muninn.RenderEndpointDiscovery]::FriendlyNames() | Sort-Object -Unique)
  if ($audioNames.Count -lt 1) {
    $audioDeviceIndex = [Array]::IndexOf($arguments, '--audio-device')
    if ($audioDeviceIndex -ge 0 -and ($audioDeviceIndex + 1) -lt $arguments.Count) {
      $audioNames = @($arguments[$audioDeviceIndex + 1])
    }
  }
  foreach ($audioName in $audioNames) {
    $arguments += @('--audio-source', ('wasapi-loopback:{0}={1} loopback ({0})' -f $audioName.Trim(), $hostLabel))
  }
}
'@

$activateArguments = @(
  "request-stream",
  "--store", $StorePath,
  "--activate-store", $ActivateStorePath,
  "--capture-command-rudp-target", $CaptureCommandRudpTarget,
  "--host", "raven",
  "--stream", "muninn.raven.av.rudp",
  "--stream-action", "start",
  "--target-host", $TargetHost,
  "--port", $Port.ToString(),
  "--media-transport", $MediaTransport
)
if ($MediaTransport -ne "srt" -or ($TargetHost -eq $ObsTargetHost -and $Port -eq $ObsPort)) {
  $activateArguments += "--no-obs-target"
} else {
  $activateArguments += @("--obs-target-host", $ObsTargetHost, "--obs-port", $ObsPort.ToString())
}
$activateArguments += @(
  "--audio-device", $AudioDevice,
  "--ffmpeg", $Ffmpeg,
  "--loopback-script", $LoopbackScript,
  "--log-root", $LogRoot
)

$videoProofFramerate = 30
$videoProofBitrateKbps = 12000
$videoProofVbvKbits = [Math]::Ceiling($videoProofBitrateKbps / $videoProofFramerate)

$videoProofArguments = @(
  "-hide_banner",
  "-loglevel", "info",
  "-fflags", "nobuffer",
  "-flags", "low_delay",
  "-thread_queue_size", "1024",
  "-f", "lavfi",
  "-i", ("ddagrab=framerate={0}:output_idx=0:draw_mouse=1" -f $videoProofFramerate),
  "-thread_queue_size", "1024",
  "-f", "lavfi",
  "-i", "anullsrc=channel_layout=stereo:sample_rate=48000",
  "-map", "0:v:0",
  "-map", "1:a:0",
  "-c:v", "h264_nvenc",
  "-preset", "p1",
  "-tune", "ull",
  "-zerolatency", "1",
  "-bf", "0",
  "-delay", "0",
  "-rc", "cbr",
  "-rc-lookahead", "0",
  "-b:v", ("{0}k" -f $videoProofBitrateKbps),
  "-maxrate", ("{0}k" -f $videoProofBitrateKbps),
  "-bufsize", ("{0}k" -f $videoProofVbvKbits),
  "-g", $videoProofFramerate.ToString(),
  "-forced-idr", "1",
  "-c:a", "aac",
  "-b:a", "192k",
  "-ar", "48000",
  "-ac", "2",
  "-f", "mpegts",
  ("srt://{0}:{1}?mode=caller&latency=120000&timeout=30000000" -f $ObsTargetHost, $ObsPort)
)

$servePsContent = New-HiddenNativeLauncherPowerShellContent `
  -StorePath $StorePath `
  -LogRoot $LogRoot `
  -Arguments $serveArguments `
  -FilePath $MuninnExe `
  -StdoutPath (Join-Path $LogRoot "muninn-serve.out.log") `
  -StderrPath (Join-Path $LogRoot "muninn-serve.err.log") `
  -PidPath (Join-Path $LogRoot "muninn-serve.pid") `
  -DynamicArgumentsScript $serveDynamicArgumentsScript

$activatePsContent = New-HiddenNativeLauncherPowerShellContent `
  -StorePath $ActivateStorePath `
  -LogRoot $LogRoot `
  -Arguments $activateArguments `
  -FilePath $MuninnExe `
  -StdoutPath (Join-Path $LogRoot "muninn-activate.out.log") `
  -StderrPath (Join-Path $LogRoot "muninn-activate.err.log") `
  -PidPath (Join-Path $LogRoot "muninn-activate.pid")

$videoProofPsContent = New-HiddenNativeLauncherPowerShellContent `
  -LogRoot $LogRoot `
  -Arguments $videoProofArguments `
  -FilePath $Ffmpeg `
  -StdoutPath (Join-Path $LogRoot "muninn-video-proof.out.log") `
  -StderrPath (Join-Path $LogRoot "muninn-video-proof.err.log") `
  -PidPath (Join-Path $LogRoot "muninn-video-proof.pid") `
  -WorkingDirectory $muninnDir

$uploadRoot = Join-Path $env:TEMP ("odin-raven-muninn-launchers-" + [guid]::NewGuid().ToString("N"))

try {
  New-Item -ItemType Directory -Force -Path $uploadRoot | Out-Null

  $localServePs = Join-Path $uploadRoot "start-muninn-serve.ps1"
  $localServeVbs = Join-Path $uploadRoot "start-muninn-serve-hidden.vbs"
  $localServeCmd = Join-Path $uploadRoot "start-muninn-serve.cmd"
  $localActivatePs = Join-Path $uploadRoot "activate-raven-av-srt.ps1"
  $localActivateVbs = Join-Path $uploadRoot "activate-raven-av-srt-hidden.vbs"
  $localActivateCmd = Join-Path $uploadRoot "activate-raven-av-srt.cmd"
  $localVideoProofPs = Join-Path $uploadRoot "muninn-raven-video-to-starfire-obs.ps1"
  $localVideoProofVbs = Join-Path $uploadRoot "muninn-raven-video-to-starfire-obs-hidden.vbs"
  $localVideoProofCmd = Join-Path $uploadRoot "muninn-raven-video-to-starfire-obs.cmd"

  Set-AsciiFile -Path $localServePs -Content $servePsContent
  Set-AsciiFile -Path $localServeVbs -Content (New-HiddenPowerShellVbsLauncherContent -PsPath $servePsPath)
  Set-AsciiFile -Path $localServeCmd -Content (New-WscriptCmdLauncherContent -WorkingDirectory $muninnDir -VbsPath $serveVbsPath)
  Set-AsciiFile -Path $localActivatePs -Content $activatePsContent
  Set-AsciiFile -Path $localActivateVbs -Content (New-HiddenPowerShellVbsLauncherContent -PsPath $activatePsPath)
  Set-AsciiFile -Path $localActivateCmd -Content (New-WscriptCmdLauncherContent -WorkingDirectory $muninnDir -VbsPath $activateVbsPath)
  Set-AsciiFile -Path $localVideoProofPs -Content $videoProofPsContent
  Set-AsciiFile -Path $localVideoProofVbs -Content (New-HiddenPowerShellVbsLauncherContent -PsPath $videoProofPsPath)
  Set-AsciiFile -Path $localVideoProofCmd -Content (New-WscriptCmdLauncherContent -WorkingDirectory $muninnDir -VbsPath $videoProofVbsPath)

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
  "$serveVbsPath",
  "$activatePsPath",
  "$activateVbsPath",
  "$videoProofPsPath",
  "$videoProofVbsPath"
)) {
  if (-not (Test-Path -LiteralPath `$path)) {
    throw "Required launcher path not found at `$path"
  }
}

Get-CimInstance Win32_Process |
  Where-Object { `$_.Name -like "muninn*.exe" } |
  ForEach-Object {
    & cmd.exe /c ("taskkill /PID {0} /T /F >nul 2>&1" -f `$_.ProcessId) | Out-Null
  }

New-Item -ItemType Directory -Force -Path "$LogRoot" | Out-Null
New-Item -ItemType Directory -Force -Path (Split-Path -Parent "$StorePath") | Out-Null
& netsh.exe advfirewall firewall delete rule name="GameCult Muninn Capture Command RUDP" | Out-Null 2>`$null
& netsh.exe advfirewall firewall add rule name="GameCult Muninn Capture Command RUDP" dir=in action=allow protocol=UDP localport=17873 | Out-Null

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
Register-HiddenVbsTask -TaskName "GameCult-Muninn-Activate" -VbsPath "$activateVbsPath"
Register-HiddenVbsTask -TaskName "GameCult-Muninn-VideoProof" -VbsPath "$videoProofVbsPath"

Assert-HiddenPowerShellTask -TaskName "GameCult-Muninn" -PsPath "$servePsPath"
Assert-HiddenVbsTask -TaskName "GameCult-Muninn-Activate" -VbsPath "$activateVbsPath" -PsPath "$activatePsPath"
Assert-HiddenVbsTask -TaskName "GameCult-Muninn-VideoProof" -VbsPath "$videoProofVbsPath" -PsPath "$videoProofPsPath"

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
    @{ LocalPath = $localServePs; RemotePath = ($servePsPath -replace "\\", "/") },
    @{ LocalPath = $localServeVbs; RemotePath = ($serveVbsPath -replace "\\", "/") },
    @{ LocalPath = $localServeCmd; RemotePath = ($serveCmdPath -replace "\\", "/") },
    @{ LocalPath = $localActivatePs; RemotePath = ($activatePsPath -replace "\\", "/") },
    @{ LocalPath = $localActivateVbs; RemotePath = ($activateVbsPath -replace "\\", "/") },
    @{ LocalPath = $localActivateCmd; RemotePath = ($activateCmdPath -replace "\\", "/") },
    @{ LocalPath = $localVideoProofPs; RemotePath = ($videoProofPsPath -replace "\\", "/") },
    @{ LocalPath = $localVideoProofVbs; RemotePath = ($videoProofVbsPath -replace "\\", "/") },
    @{ LocalPath = $localVideoProofCmd; RemotePath = ($videoProofCmdPath -replace "\\", "/") }
  )

  Invoke-RavenUploadedPowerShell -RavenHost $RavenHost -RemoteScriptContent $remoteScript -UploadSpecs $uploadSpecs -TempPrefix "odin-raven-muninn-restart"
} finally {
  Remove-Item -LiteralPath $uploadRoot -Recurse -Force -ErrorAction SilentlyContinue
}

$restartExit = $LASTEXITCODE
if ($restartExit -eq 0) {
  Start-Sleep -Seconds 2
  $healthScript = Join-Path $PSScriptRoot "health-muninn.ps1"
  & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $healthScript `
    -RavenHost $RavenHost `
    -MuninnExe $MuninnExe `
    -StorePath $StorePath `
    -ConnectTimeoutSeconds $ConnectTimeoutSeconds `
    -MaxStoreAgeSeconds ([Math]::Max(180, $ServeStartTimeoutSeconds + 60)) `
    -SshUser $SshUser `
    -IdentityFile $IdentityFile
}
exit $restartExit
