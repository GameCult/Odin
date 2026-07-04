$repoRoot = Split-Path -Parent $PSScriptRoot

$IdunnDeploymentSharedActuators = @(
  "$repoRoot\scripts\deploy-yggdrasil-source-app.ps1"
)

$IdunnDeploymentTargets = @(
  [pscustomobject]@{
    Id = "odin"
    Repo = "Odin"
    LocalPath = "E:\Projects\Odin"
    Host = "starfire"
    Service = "odin-coordinator"
    Status = "runtime-enforced"
    Health = "daemon-published-rudp:odin.cultnet-rudp-provider-health"
    Deploy = $null
    Restart = "$repoRoot\scripts\restart-odin.ps1"
    Aliases = @("$repoRoot\scripts\restart-odin.cmd")
    Reason = "Central Idunn keeps Odin's all-seer/rendezvous daemon alive; Odin owns Verse discovery while Idunn owns lifecycle actuation."
  },
  [pscustomobject]@{
    Id = "idunn"
    Repo = "Odin"
    LocalPath = "E:\Projects\Odin"
    Host = "starfire"
    Service = "idunn swarm supervisor"
    Status = "runtime-enforced"
    Health = $null
    Deploy = $null
    Restart = $null
    Aliases = @()
    Reason = "start-idunn-local.ps1 is the bootstrap for Idunn itself; it is not a deploy/restart actuator over another daemon."
  },
  [pscustomobject]@{
    Id = "stonks"
    Repo = "Stonks"
    LocalPath = "E:\Projects\Stonks"
    Host = "starfire"
    Service = "stonks daemon"
    Status = "runtime-enforced"
    Health = "daemon-published-rudp:stonks.cultnet-rudp-market-health"
    Deploy = $null
    Restart = "$repoRoot\scripts\restart-stonks.cmd"
    Aliases = @()
    Reason = "Stonks publishes market/provider state and daemon health through CultMesh/CultNet; local restart is an Idunn actuator only."
  },
  [pscustomobject]@{
    Id = "weksa"
    Repo = "weksa"
    LocalPath = "E:\Projects\weksa"
    Host = "starfire"
    Service = "weksa daemon"
    Status = "runtime-enforced"
    Health = "daemon-published-rudp:weksa.cultnet-rudp-provider-health"
    Deploy = $null
    Restart = "$repoRoot\scripts\restart-weksa.cmd"
    Aliases = @()
    Reason = "Weksa provider state and health are daemon-owned CultMesh/CultNet records; local restart is an Idunn actuator only."
  },
  [pscustomobject]@{
    Id = "voidbot"
    Repo = "VoidBot"
    LocalPath = "E:\Projects\VoidBot"
    Host = "starfire"
    Service = "VoidBot local stack"
    Status = "runtime-enforced"
    Health = "daemon-published-rudp:voidbot.cultnet-rudp-stack-health"
    Deploy = $null
    Restart = "$repoRoot\scripts\restart-voidbot.cmd"
    Aliases = @()
    Reason = "VoidBot stack health and provider state publish through daemon-owned CultMesh/CultNet records; restart is an Idunn actuator only."
  },
  [pscustomobject]@{
    Id = "starfire-muninn"
    Repo = "Odin"
    LocalPath = "E:\Projects\Odin"
    Host = "starfire"
    Service = "muninn serve --host starfire"
    Status = "runtime-enforced"
    Health = "daemon-published-rudp:muninn.cultnet-rudp-local-telemetry-and-quest-access"
    Deploy = $null
    Restart = "$repoRoot\scripts\restart-starfire-muninn.ps1"
    Aliases = @("$repoRoot\scripts\restart-starfire-muninn.cmd")
    Reason = "Central Idunn keeps Starfire-local Muninn alive for Quest access and local telemetry surfaces."
  },
  [pscustomobject]@{
    Id = "raven-muninn"
    Repo = "Odin"
    LocalPath = "E:\Projects\Odin"
    Host = "raven"
    Service = "muninn serve --host raven"
    Status = "runtime-enforced"
    Health = "daemon-published-rudp:muninn.cultnet-rudp-remote-telemetry-health"
    Deploy = $null
    Restart = "$repoRoot\scripts\restart-muninn.ps1"
    Aliases = @("$repoRoot\scripts\restart-muninn.cmd")
    Reason = "Central Idunn keeps Raven Muninn's remote telemetry posture alive without activating A/V capture as part of keepalive."
  },
  [pscustomobject]@{
    Id = "nightwing-muninn"
    Repo = "Odin"
    LocalPath = "E:\Projects\Odin"
    Host = "nightwing"
    Service = "muninn serve --host nightwing"
    Status = "runtime-enforced"
    Health = "daemon-published-rudp:muninn.cultnet-rudp-remote-telemetry-and-move-hid"
    Deploy = $null
    Restart = "$repoRoot\scripts\restart-nightwing-muninn.ps1"
    Aliases = @("$repoRoot\scripts\restart-nightwing-muninn.cmd")
    Reason = "Central Idunn reaches Nightwing over SSH and keeps the host-local Muninn Move HID daemon alive."
  },
  [pscustomobject]@{
    Id = "vili"
    Repo = "Vili"
    LocalPath = "E:\Projects\Vili"
    Host = "raven"
    Service = "GameCult\\Vili"
    Status = "runtime-enforced"
    Health = "daemon-published-rudp:vili.cultnet-rudp-animation-health"
    Deploy = $null
    Restart = "$repoRoot\scripts\restart-vili.cmd"
    Aliases = @("$repoRoot\scripts\restart-vili.ps1")
    Reason = "Central Idunn keeps Raven Vili's typed motion daemon alive; command requests and health are CultMesh/CultNet-owned."
  },
  [pscustomobject]@{
    Id = "raven-sleipnir"
    Repo = "Odin"
    LocalPath = "E:\Projects\Odin"
    Host = "raven"
    Service = "sleipnir --host raven"
    Status = "runtime-enforced"
    Health = "daemon-published-rudp:sleipnir.cultnet-rudp-input-mirror-health"
    Deploy = "$repoRoot\scripts\deploy-raven-sleipnir.ps1"
    Restart = "$repoRoot\scripts\restart-raven-sleipnir.ps1"
    Aliases = @()
    Reason = "Central Idunn reaches Raven over SSH and keeps Sleipnir's hidden scheduled task alive while Sleipnir publishes daemon-owned RUDP health."
  },
  [pscustomobject]@{
    Id = "nightwing-gjallar"
    Repo = "Gjallar"
    LocalPath = "E:\Projects\Gjallar"
    Host = "nightwing"
    Service = "gjallar.service"
    Status = "enforced"
    Health = "daemon-published-rudp:gjallar.cultnet-rudp-framebuffer-composition-health"
    Deploy = "$repoRoot\scripts\deploy-nightwing-gjallar.ps1"
    Restart = "$repoRoot\scripts\restart-nightwing-gjallar.ps1"
    Aliases = @("$repoRoot\scripts\deploy-nightwing-gjallar.cmd", "$repoRoot\scripts\restart-nightwing-gjallar.cmd")
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
    Aliases = @()
    Reason = "Committed Bifrost HEAD currently expects UserAccounts.HeimdallAccountId, but Yggdrasil's production database lacks that column and EF reports no pending migration. Idunn must not enforce Bifrost freshness until the Bifrost schema migration path is fixed."
  },
  [pscustomobject]@{
    Id = "yggdrasil-streampixels"
    Repo = "StreamPixels"
    LocalPath = "E:\Projects\StreamPixels"
    Host = "yggdrasil"
    Service = "streampixels-service/streampixels-web"
    Status = "enforced"
    Health = "daemon-published-rudp:streampixels.cultnet-rudp-service-health"
    Deploy = "$repoRoot\scripts\deploy-yggdrasil-streampixels.cmd"
    Restart = $null
    Aliases = @("$repoRoot\scripts\deploy-yggdrasil-streampixels.ps1")
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
    Health = "daemon-published-rudp:heimdall.cultnet-rudp-provider-health"
    Deploy = "$repoRoot\scripts\deploy-yggdrasil-heimdall.cmd"
    Restart = $null
    Aliases = @("$repoRoot\scripts\deploy-yggdrasil-heimdall.ps1")
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
    Health = "daemon-published-rudp:repixelizer.cultnet-rudp-service-health"
    Deploy = "$repoRoot\scripts\deploy-yggdrasil-repixelizer.cmd"
    Restart = $null
    Aliases = @("$repoRoot\scripts\deploy-yggdrasil-repixelizer.ps1")
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
    Aliases = @()
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
    Aliases = @()
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
    Aliases = @()
    Reason = "Android USB install requires on-device approval; Idunn must not claim automatic deploy authority until that boundary is solved."
  },
  [pscustomobject]@{
    Id = "nightwing-eve-dashboard"
    Repo = "Mimir"
    LocalPath = "E:\Projects\Mimir"
    Host = "nightwing"
    Service = "nightwing-eve-dashboard.service"
    Status = "archived"
    Health = $null
    Deploy = $null
    Restart = $null
    Aliases = @("$repoRoot\scripts\restart-nightwing-eve-dashboard.cmd")
    Reason = "Nightwing Eve dashboard's old HTTP/WebSocket broker is archived. Rebuild it only as typed CultMesh/Odin state publication plus renderer lowering before restoring Idunn lifecycle authority."
  },
  [pscustomobject]@{
    Id = "nightwing-eve-browser-reference"
    Repo = "Eve"
    LocalPath = "E:\Projects\Eve"
    Host = "nightwing"
    Service = "nightwing-eve-browser-reference.service"
    Status = "runtime-enforced"
    Health = "daemon-published-rudp:nightwing.cultnet-rudp-browser-reference-health"
    Deploy = $null
    Restart = "$repoRoot\scripts\restart-nightwing-eve-browser-reference.ps1"
    Aliases = @("$repoRoot\scripts\restart-nightwing-eve-browser-reference.cmd")
    Reason = "Nightwing Eve browser reference publishes daemon-owned boundary witnesses and RUDP health; systemd restart is an Idunn actuator only."
  }
)
