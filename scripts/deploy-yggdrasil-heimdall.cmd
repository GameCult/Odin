@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File E:\Projects\Odin\scripts\deploy-yggdrasil-source-app.ps1 -AppId yggdrasil-heimdall -RepoRoot E:\Projects\Heimdall -AppUser heimdall -RemoteAppHome /srv/heimdall -RemoteTarballName heimdall-source.tar -DeployScript E:\Projects\gamecult-ops\scripts\deploy-heimdall.sh -CheckScript E:\Projects\gamecult-ops\scripts\check-heimdall.sh
