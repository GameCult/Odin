@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File E:\Projects\Odin\scripts\health-yggdrasil-source-app.ps1 -AppId yggdrasil-repixelizer -RepoRoot E:\Projects\repixelizer -RemoteAppHome /srv/repixelizer -CheckScript E:\Projects\gamecult-ops\scripts\check-repixelizer-gui.sh
