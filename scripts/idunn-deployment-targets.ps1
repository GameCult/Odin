$repoRoot = Split-Path -Parent $PSScriptRoot
$opsRoot = "E:\Projects\gamecult-ops"

$IdunnDeploymentTargets = @(
  [pscustomobject]@{
    Id = "nightwing-gjallar"
    Repo = "Gjallar"
    LocalPath = "E:\Projects\Gjallar"
    Host = "nightwing"
    Service = "gjallar.service"
    Status = "enforced"
    Health = "$repoRoot\scripts\health-nightwing-gjallar.cmd"
    Deploy = "$repoRoot\scripts\deploy-nightwing-gjallar.cmd"
    Restart = "$repoRoot\scripts\restart-nightwing-gjallar.cmd"
    Reason = "Nightwing framebuffer compositor has a committed artifact manifest and automatic Idunn deploy lane."
  },
  [pscustomobject]@{
    Id = "yggdrasil-bifrost"
    Repo = "Bifrost"
    LocalPath = "E:\Projects\Bifrost"
    Host = "yggdrasil"
    Service = "bifrost.service/bifrost-web"
    Status = "blocked"
    Health = "$opsRoot\scripts\check-bifrost-alpha.sh"
    Deploy = "$opsRoot\scripts\deploy-bifrost-container.ps1"
    Restart = $null
    Reason = "Deploy authority exists in gamecult-ops, but desired image digest/tag is not yet published as an Idunn deployment target record."
  },
  [pscustomobject]@{
    Id = "yggdrasil-streampixels"
    Repo = "StreamPixels"
    LocalPath = "E:\Projects\StreamPixels"
    Host = "yggdrasil"
    Service = "streampixels-service/streampixels-web"
    Status = "blocked"
    Health = "$opsRoot\scripts\check-streampixels-preview.sh"
    Deploy = "$opsRoot\scripts\deploy-streampixels-preview.sh"
    Restart = $null
    Reason = "Deploy script is remote-root/runbook shaped and needs a committed artifact manifest plus Idunn-safe wrapper before automatic execution."
  },
  [pscustomobject]@{
    Id = "yggdrasil-heimdall"
    Repo = "Heimdall"
    LocalPath = "E:\Projects\Heimdall"
    Host = "yggdrasil"
    Service = "heimdall.service"
    Status = "blocked"
    Health = "$opsRoot\scripts\check-heimdall.sh"
    Deploy = "$opsRoot\scripts\deploy-heimdall.sh"
    Restart = $null
    Reason = "Deploy script uses a workstation-built source tarball; Idunn needs a manifest-producing wrapper before automatic execution."
  },
  [pscustomobject]@{
    Id = "yggdrasil-repixelizer"
    Repo = "repixelizer"
    LocalPath = "E:\Projects\repixelizer"
    Host = "yggdrasil"
    Service = "repixelizer-gui.service"
    Status = "blocked"
    Health = "$opsRoot\scripts\check-repixelizer-gui.sh"
    Deploy = "$opsRoot\scripts\deploy-repixelizer-gui.sh"
    Restart = $null
    Reason = "Deploy script uses a committed source tarball and Python environment mutation; Idunn needs an artifact manifest wrapper before automatic execution."
  },
  [pscustomobject]@{
    Id = "github-pages-gamecult-site"
    Repo = "gamecult-site"
    LocalPath = "E:\Projects\gamecult-site"
    Host = "github-pages"
    Service = "gamecult.org/www"
    Status = "external-owned"
    Health = $null
    Deploy = $null
    Restart = $null
    Reason = "GitHub Actions/Pages owns deployment freshness; Idunn should ingest workflow/deployment status before claiming automatic deploy authority."
  },
  [pscustomobject]@{
    Id = "eve-ipad-evecanvas"
    Repo = "Eve"
    LocalPath = "E:\Projects\Eve"
    Host = "eve-ipad"
    Service = "org.gamecult.evecanvas"
    Status = "blocked"
    Health = $null
    Deploy = $null
    Restart = $null
    Reason = "Jailbroken iPad native app deployment exists only as an operator runbook; no safe noninteractive Idunn deploy command yet."
  },
  [pscustomobject]@{
    Id = "periwinkle-eve-android"
    Repo = "Eve"
    LocalPath = "E:\Projects\Eve"
    Host = "periwinkle"
    Service = "Eve Android proof APK"
    Status = "blocked"
    Health = $null
    Deploy = $null
    Restart = $null
    Reason = "Android USB install requires on-device approval; Idunn must not claim automatic deploy authority until that boundary is solved."
  }
)
