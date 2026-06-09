@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File E:\Projects\Odin\scripts\health-yggdrasil-source-app.ps1 -AppId yggdrasil-streampixels -RepoRoot E:\Projects\StreamPixels -RemoteAppHome /srv/streampixels -CheckScript E:\Projects\gamecult-ops\scripts\check-streampixels-preview.sh
