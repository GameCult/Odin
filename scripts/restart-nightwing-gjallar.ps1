param(
  [string] $SshTarget = "nwroot",
  [string] $Service = "gjallar.service"
)

$ErrorActionPreference = "Stop"

if ($env:IDUNN_ACTUATOR -ne "1" -or $env:IDUNN_COMMAND_AUTHORITY -ne "idunn-daemon") {
  throw "restart-nightwing-gjallar.ps1 is an Idunn actuator body. Redeploy by poking Idunn; direct service restart is not an owned path."
}

ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget "systemctl restart $Service && systemctl is-active --quiet $Service"
if ($LASTEXITCODE -ne 0) {
  throw "Remote restart failed for $Service on $SshTarget with exit code $LASTEXITCODE"
}

Write-Host "Restarted $Service on $SshTarget."
