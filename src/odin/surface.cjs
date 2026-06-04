"use strict";

const { analyzeElementTree, fullscreenLayoutIntent, mergeLayoutIntent } = require("./layout.cjs");
const { observationPane } = require("./observations.cjs");
const { stableId } = require("./utils.cjs");

function buildSurface({ observedAt, docker, adb, hosts, yggdrasilServices, verses, interfaces, observations, layout }) {
  const activeInterfaces = interfaces.filter(hasOverviewSignal);
  const activeObservationStreams = observations.streams.filter((entry) => entry.state === "active");
  return {
    schema: "gamecult.eve.surface.v1",
    id: "gamecult.network.status.surface",
    title: "Odin",
    root: {
      id: "network-root",
      kind: "dashboard",
      props: {
        title: "Odin All-Seer",
        observedAt,
        summary: `${activeInterfaces.length} Eve surfaces / ${activeObservationStreams.length} live streams`,
        layout: fullscreenLayoutIntent("odin.allseer", -100),
        presentation: {
          theme: "bifrost-pride",
          frame: "animated-rainbow",
          dividerMotion: "bifrost-rainbow-dance",
          intent: "Nightwing Odin Overview should render with an animated rainbow frame and dancing dividers; renderer owns lowering, Odin owns truth.",
        },
      },
      children: [
        ...activeInterfaces.map((entry) => {
          const tree = analyzeElementTree(entry.surface.root);
          return {
            id: `interface-${stableId(entry.providerId)}`,
            kind: "interface",
            props: {
              title: verseUri(entry),
              providerId: entry.providerId,
              source: entry.source,
              status: entry.state,
              detail: entry.detail,
              version: entry.version,
              updatedAt: entry.updatedAt,
              layout: mergeLayoutIntent(layout.tiles?.[entry.providerId], entry, interfaces),
              tree,
              packing: {
                strategy: "nested-dense-signal",
                flattening: "forbidden-when-surface-root-has-children",
                listPolicy: "group-or-tree; never render provider signal as a single log/list blob",
              },
            },
            children: [entry.surface.root],
          };
        }),
      ],
    },
    assets: [],
  };
}

function hasOverviewSignal(entry) {
  const root = entry?.surface?.root;
  if (!root) return false;

  const providerId = String(entry.providerId || "").toLowerCase();
  const explicit = root.props?.overview || entry.manifest?.overview;
  if (explicit?.visible === false) return false;
  if (explicit?.visible === true || explicit?.signal === "live-ops") return true;

  const text = surfaceText(root);
  if (providerId.includes("voidbot.swarm")) return /\bctb\b|\bs[0-9.]+\s+h[0-9.]+/i.test(text);
  if (providerId.includes("mimir.stream.layout")) return false;
  if (providerId.includes("spotiverse")) return spotiverseHasOverviewSignal(root, text);
  if (providerId.includes("streampixels")) return streamPixelsHasOverviewSignal(root, text);
  if (providerId.includes("mimir.live.stats")) return mimirHasOverviewSignal(root, text);

  if (root.props?.compatibility === "legacy-dashboard-nodes") return false;
  if (/\b(rate limit|playing:\s*none|queue:\s*empty|unavailable|not discovered)\b/i.test(text)) return false;
  return hasMetricElement(root) || /\b(users?|viewers?|events?|traffic|ingest|throughput|latency|dropout|queue|pressure|saturation|error|fault|warning)\b/i.test(text);
}

function spotiverseHasOverviewSignal(root, text) {
  const status = String(root.props?.status || "").toLowerCase();
  if (status === "warn" && /\brate limit\b/i.test(text)) return false;
  if (/\bplaying:\s*none\b/i.test(text)) return false;
  if (/\bqueue:\s*empty|queue:\s*empty or unavailable\b/i.test(text)) return false;
  return /\bplaying:\s*(?!none\b).+|\bqueue:\s*(?!empty\b|0\b).+|\brequests?:\s*[1-9]|\bvolume:\s*[1-9]/i.test(text);
}

function streamPixelsHasOverviewSignal(root, text) {
  if (root.props?.compatibility === "legacy-dashboard-nodes" && !/\b(users?|viewers?|events?|traffic|ingest|throughput|latency|queue|pressure|saturation|dropped|errors?)\b/i.test(text)) {
    return false;
  }

  return [
    /\b(live\s*)?(users?|viewers?)\s*[:=]?\s*[1-9]/i,
    /\bevents?(\s*traffic|\s*\/\s*s|\s*per\s*sec)?\s*[:=]?\s*[1-9]/i,
    /\bingest\b.*\b(saturation|pressure|traffic|throughput|[1-9][0-9]*(\.[0-9]+)?)\b/i,
    /\b(queue|backlog|pressure)\s*[:=]?\s*[1-9]/i,
    /\b(errors?|dropped|5xx)\s*[:=]?\s*[1-9]/i,
    /\bthroughput\s*[:=]?\s*[1-9]/i,
  ].some((pattern) => pattern.test(text));
}

function mimirHasOverviewSignal(root, text) {
  if (root.props?.compatibility === "legacy-dashboard-nodes") return false;
  return [
    /\bdropout\b.*\b[1-9]/i,
    /\bstale\s+streams?\s*[:=]?\s*[1-9]/i,
    /\blatency\b.*\b([2-9][0-9]{2,}|[1-9]\d+\.\d+ms)\b/i,
    /\bconfidence\b.*\b0\.[0-4]/i,
  ].some((pattern) => pattern.test(text));
}

function surfaceText(root) {
  const parts = [];
  visitSurface(root, (node) => {
    const props = node.props || {};
    for (const key of ["title", "label", "text", "status", "detail", "summary", "value"]) {
      if (props[key] !== undefined && props[key] !== null) parts.push(String(props[key]));
    }
  });
  return parts.join("\n");
}

function hasMetricElement(root) {
  let found = false;
  visitSurface(root, (node) => {
    if (node.kind === "metric") found = true;
  });
  return found;
}

function visitSurface(node, visitor) {
  if (!node || typeof node !== "object") return;
  visitor(node);
  for (const child of Array.isArray(node.children) ? node.children : []) {
    visitSurface(child, visitor);
  }
}

function buildPendingSurface(message) {
  return {
    schema: "gamecult.eve.surface.v1",
    id: "gamecult.network.status.surface",
    title: "Odin",
    root: stack("root", [pane("Coordinator", [text("status", message)])]),
    assets: [],
  };
}

function stack(id, children) {
  return { id, kind: "stack", props: {}, children };
}

function pane(title, children) {
  return { id: stableId(`pane-${title}`), kind: "pane", props: { title }, children };
}

function card(id, children, tone = "default") {
  return { id, kind: "card", props: { title: id }, style: { tone }, children };
}

function text(id, value) {
  return { id, kind: "text", props: { text: value }, children: [] };
}

function metric(id, label, value, tone) {
  return { id, kind: "metric", props: { label, text: `${label}: ${value}`, value, tone }, children: [] };
}

function verseUri(entry) {
  const providerId = String(entry.providerId || "").trim();
  if (hasBodyPrefix(providerId)) return providerId;
  const body = sourceBody(entry.source);
  if (!body || providerId.startsWith(`${body}.`)) return providerId;
  return `${body}.${providerId}`;
}

function hasBodyPrefix(providerId) {
  return ["starfire", "yggdrasil", "nightwing", "eve", "periwinkle", "raven"].some((body) => providerId.startsWith(`${body}.`));
}

function sourceBody(source) {
  const value = String(source || "");
  if (value.startsWith("cultmesh:")) return "starfire";

  try {
    const url = new URL(value);
    const host = url.hostname.toLowerCase();
    if (host === "127.0.0.1" || host === "localhost" || host === "::1" || host === "172.17.0.1" || host === "192.168.1.66") {
      return "starfire";
    }
    if (host === "192.168.1.75") return "eve";
  } catch {
    // Unknown source strings keep the provider-owned id unchanged.
  }

  return "";
}

function adbTone(adb) {
  if (adb.state !== "ok") return "warn";
  return adb.devices.some((device) => device.state !== "device") ? "warn" : "ok";
}

module.exports = {
  buildPendingSurface,
  buildSurface,
  card,
  hasOverviewSignal,
  metric,
  pane,
  stack,
  text,
};
