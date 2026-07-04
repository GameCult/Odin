@echo off
if not "%IDUNN_ACTUATOR%"=="1" (
  echo restart-voidbot.cmd is an Idunn actuator body. Redeploy by poking Idunn; direct service restart is not an owned path. 1>&2
  exit /b 1
)
if /I not "%IDUNN_COMMAND_AUTHORITY%"=="idunn-daemon" (
  echo restart-voidbot.cmd is an Idunn actuator body. Redeploy by poking Idunn; direct service restart is not an owned path. 1>&2
  exit /b 1
)
powershell.exe -NoProfile -ExecutionPolicy Bypass -File E:\Projects\VoidBot\scripts\start-voidbot-stack.ps1
