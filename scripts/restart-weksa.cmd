@echo off
if not "%IDUNN_ACTUATOR%"=="1" (
  echo restart-weksa.cmd is an Idunn actuator body. Redeploy by poking Idunn; direct service restart is not an owned path. 1>&2
  exit /b 1
)
if /I not "%IDUNN_COMMAND_AUTHORITY%"=="idunn-daemon" (
  echo restart-weksa.cmd is an Idunn actuator body. Redeploy by poking Idunn; direct service restart is not an owned path. 1>&2
  exit /b 1
)
call E:\Projects\weksa\scripts\restart-weksa-daemon.cmd
