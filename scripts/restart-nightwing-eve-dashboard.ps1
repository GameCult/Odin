param(
  [string] $SshTarget = "nwroot",
  [string] $Service = "nightwing-eve-dashboard.service"
)

$ErrorActionPreference = "Stop"

ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget "systemctl restart $Service && systemctl is-active --quiet $Service"
if ($LASTEXITCODE -ne 0) {
  throw "Remote restart failed for $Service on $SshTarget with exit code $LASTEXITCODE"
}

Write-Host "Restarted $Service on $SshTarget."
