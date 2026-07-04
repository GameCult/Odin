@echo off
if not "%IDUNN_ACTUATOR%"=="1" (
  echo restart-nightwing-gjallar.cmd is an Idunn actuator wrapper. Redeploy by poking Idunn; direct service restart is not an owned path. 1>&2
  exit /b 1
)
if /I not "%IDUNN_COMMAND_AUTHORITY%"=="idunn-daemon" (
  echo restart-nightwing-gjallar.cmd is an Idunn actuator wrapper. Redeploy by poking Idunn; direct service restart is not an owned path. 1>&2
  exit /b 1
)
powershell.exe -NoProfile -ExecutionPolicy Bypass -File E:\Projects\Odin\scripts\restart-nightwing-gjallar.ps1
