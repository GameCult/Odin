param(
  [string] $FfmpegRoot = $env:MUNINN_FFMPEG_ROOT,
  [string] $OutputRoot = (Join-Path $PSScriptRoot '..\artifacts\muninn-video-encoder\bundle')
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path $PSScriptRoot -Parent
$sourceRoot = Join-Path $repoRoot 'native\muninn-video-encoder'

if ([string]::IsNullOrWhiteSpace($FfmpegRoot)) {
  $candidate = 'E:\Projects\Mimir\artifacts\obs-sdk\obs-plugintemplate\.deps\obs-deps-2025-07-11-x64'
  if (Test-Path -LiteralPath $candidate) {
    $FfmpegRoot = $candidate
  }
}
if ([string]::IsNullOrWhiteSpace($FfmpegRoot) -or
    -not (Test-Path -LiteralPath (Join-Path $FfmpegRoot 'include\libavcodec\avcodec.h'))) {
  throw 'Set -FfmpegRoot or MUNINN_FFMPEG_ROOT to an FFmpeg development root containing include/ and lib/.'
}

$buildRoot = Join-Path $sourceRoot 'build'
cmake -S $sourceRoot -B $buildRoot "-DFFMPEG_ROOT=$($FfmpegRoot.Replace('\', '/'))"
if ($LASTEXITCODE -ne 0) { throw "CMake configure failed with $LASTEXITCODE" }
cmake --build $buildRoot --config Release
if ($LASTEXITCODE -ne 0) { throw "CMake build failed with $LASTEXITCODE" }

New-Item -ItemType Directory -Force -Path $OutputRoot | Out-Null
Copy-Item -LiteralPath (Join-Path $buildRoot 'Release\muninn-video-encoder.exe') -Destination $OutputRoot -Force
Get-ChildItem -LiteralPath (Join-Path $FfmpegRoot 'bin') -Filter '*.dll' |
  Copy-Item -Destination $OutputRoot -Force

$exe = Join-Path $OutputRoot 'muninn-video-encoder.exe'
if (-not (Test-Path -LiteralPath $exe)) { throw 'Encoder bundle is missing its executable.' }
Write-Host "Muninn controllable encoder bundle: $OutputRoot"
