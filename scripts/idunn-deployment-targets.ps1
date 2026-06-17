$repoRoot = Split-Path -Parent $PSScriptRoot

$IdunnDeploymentTargets = @(
  [pscustomobject]@{
    Id = "starfire-muninn"
    Repo = "Odin"
    LocalPath = "E:\Projects\Odin"
    Host = "starfire"
    Service = "muninn serve --host starfire"
    Status = "runtime-enforced"
    Health = "$repoRoot\scripts\health-starfire-muninn.cmd"
    Deploy = $null
    Restart = "$repoRoot\scripts\restart-starfire-muninn.cmd"
    Reason = "Central Idunn keeps Starfire-local Muninn alive for Quest access and local telemetry surfaces."
  },
  [pscustomobject]@{
    Id = "nightwing-muninn"
    Repo = "Odin"
    LocalPath = "E:\Projects\Odin"
    Host = "nightwing"
    Service = "muninn serve --host nightwing"
    Status = "runtime-enforced"
    Health = "$repoRoot\scripts\health-nightwing-muninn.cmd"
    Deploy = $null
    Restart = "$repoRoot\scripts\restart-nightwing-muninn.cmd"
    Reason = "Central Idunn reaches Nightwing over SSH and keeps the host-local Muninn Move HID daemon alive."
  },
  [pscustomobject]@{
    Id = "nightwing-gjallar"
    Repo = "Gjallar"
    LocalPath = "E:\Projects\Gjallar"
    Host = "nightwing"
    Service = "gjallar.service"
    Status = "enforced"
    Health = "$repoRoot\scripts\health-nightwing-gjallar.ps1"
    Deploy = "$repoRoot\scripts\deploy-nightwing-gjallar.ps1"
    Restart = "$repoRoot\scripts\restart-nightwing-gjallar.ps1"
    UpstreamRemote = "origin"
    UpstreamBranch = "main"
    RolloutStrategy = "restart-after-verified-build"
    StateMigration = "daemon-owned-noop"
    ZeroDowntime = "restart-required"
    Reason = "Nightwing framebuffer compositor has a committed artifact manifest and automatic Idunn deploy lane."
  },
  [pscustomobject]@{
    Id = "yggdrasil-bifrost"
    Repo = "Bifrost"
    LocalPath = "E:\Projects\Bifrost"
    Host = "yggdrasil"
    Service = "bifrost.service"
    Status = "blocked"
    Health = $null
    Deploy = $null
    Restart = $null
    Reason = "Committed Bifrost HEAD currently expects UserAccounts.HeimdallAccountId, but Yggdrasil's production database lacks that column and EF reports no pending migration. Idunn must not enforce Bifrost freshness until the Bifrost schema migration path is fixed."
  },
  [pscustomobject]@{
    Id = "yggdrasil-streampixels"
    Repo = "StreamPixels"
    LocalPath = "E:\Projects\StreamPixels"
    Host = "yggdrasil"
    Service = "streampixels-service/streampixels-web"
    Status = "enforced"
    Health = "$repoRoot\scripts\health-yggdrasil-streampixels.cmd"
    Deploy = "$repoRoot\scripts\deploy-yggdrasil-streampixels.cmd"
    Restart = $null
    UpstreamRemote = "origin"
    UpstreamBranch = "main"
    RolloutStrategy = "restart-after-verified-build"
    StateMigration = "daemon-owned-noop"
    ZeroDowntime = "restart-required"
    Reason = "Idunn packages upstream origin/main for StreamPixels, runs the existing ops deploy/check scripts, and verifies the remote manifest."
  },
  [pscustomobject]@{
    Id = "yggdrasil-heimdall"
    Repo = "Heimdall"
    LocalPath = "E:\Projects\Heimdall"
    Host = "yggdrasil"
    Service = "heimdall.service"
    Status = "enforced"
    Health = "$repoRoot\scripts\health-yggdrasil-heimdall.cmd"
    Deploy = "$repoRoot\scripts\deploy-yggdrasil-heimdall.cmd"
    Restart = $null
    UpstreamRemote = "origin"
    UpstreamBranch = "main"
    RolloutStrategy = "restart-after-verified-build"
    StateMigration = "daemon-owned-noop"
    ZeroDowntime = "restart-required"
    Reason = "Idunn packages upstream origin/main for Heimdall, runs the existing ops deploy/check scripts, and verifies the remote manifest."
  },
  [pscustomobject]@{
    Id = "yggdrasil-repixelizer"
    Repo = "repixelizer"
    LocalPath = "E:\Projects\repixelizer"
    Host = "yggdrasil"
    Service = "repixelizer-gui.service"
    Status = "enforced"
    Health = "$repoRoot\scripts\health-yggdrasil-repixelizer.cmd"
    Deploy = "$repoRoot\scripts\deploy-yggdrasil-repixelizer.cmd"
    Restart = $null
    UpstreamRemote = "origin"
    UpstreamBranch = "main"
    RolloutStrategy = "restart-after-verified-build"
    StateMigration = "daemon-owned-noop"
    ZeroDowntime = "restart-required"
    Reason = "Idunn packages upstream origin/main for repixelizer, runs the existing ops deploy/check scripts, and verifies the remote manifest."
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
