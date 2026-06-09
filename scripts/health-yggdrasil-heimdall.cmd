@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File E:\Projects\Odin\scripts\health-yggdrasil-source-app.ps1 -AppId yggdrasil-heimdall -RepoRoot E:\Projects\Heimdall -RemoteAppHome /srv/heimdall -CheckScript E:\Projects\gamecult-ops\scripts\check-heimdall.sh
