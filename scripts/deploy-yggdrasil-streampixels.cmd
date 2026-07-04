@echo off
if not "%IDUNN_ACTUATOR%"=="1" (
  echo This deployment wrapper is an Idunn actuator. Configure Idunn; do not deploy manually.
  exit /b 1
)
if /I not "%IDUNN_COMMAND_AUTHORITY%"=="idunn-daemon" (
  echo This deployment wrapper is an Idunn actuator. Configure Idunn; do not deploy manually.
  exit /b 1
)
powershell.exe -NoProfile -ExecutionPolicy Bypass -File E:\Projects\Odin\scripts\deploy-yggdrasil-streampixels.ps1
