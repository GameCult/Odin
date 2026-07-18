[CmdletBinding()]
param([string] $SshTarget = "yggdrasil")
$ErrorActionPreference = "Stop"
if ($env:IDUNN_ACTUATOR -ne "1" -or $env:IDUNN_COMMAND_AUTHORITY -ne "idunn-daemon") {
    throw "This command is owned by Idunn's deployment actuator."
}
if ($env:IDUNN_DEPLOYMENT_REQUEST_ID -notmatch '^deploy:yggdrasil-bifrost-persona-feedback:.+') {
    throw "Idunn deployment request identity is absent or targets another daemon."
}
$requestId = $env:IDUNN_DEPLOYMENT_REQUEST_ID
if ($requestId -notmatch '^[A-Za-z0-9._:-]+$') { throw "Idunn deployment request identity contains unsafe characters." }
$remote = "sudo -n env IDUNN_ACTUATOR=1 IDUNN_COMMAND_AUTHORITY=idunn-daemon IDUNN_DEPLOYMENT_REQUEST_ID='$requestId' /usr/local/sbin/idunn-deploy-bifrost-persona-feedback-yggdrasil"
& ssh.exe $SshTarget $remote
if ($LASTEXITCODE -ne 0) { throw "Yggdrasil Bifrost Persona-feedback deployment failed with exit code $LASTEXITCODE." }
