@echo off
if not "%IDUNN_ACTUATOR%"=="1" (
  echo restart-odin.cmd is an Idunn actuator wrapper. Redeploy by poking Idunn; direct service restart is not an owned path. 1>&2
  exit /b 1
)
if /I not "%IDUNN_COMMAND_AUTHORITY%"=="idunn-daemon" (
  echo restart-odin.cmd is an Idunn actuator wrapper. Redeploy by poking Idunn; direct service restart is not an owned path. 1>&2
  exit /b 1
)
"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File E:\Projects\Odin\scripts\restart-odin.ps1
