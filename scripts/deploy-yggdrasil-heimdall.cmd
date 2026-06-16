@echo off
if not "%IDUNN_ACTUATOR%"=="1" (
  echo This deployment wrapper is an Idunn actuator. Configure Idunn; do not deploy manually.
  exit /b 1
)
powershell.exe -NoProfile -ExecutionPolicy Bypass -File E:\Projects\Odin\scripts\deploy-yggdrasil-heimdall.ps1
