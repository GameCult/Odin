"use strict";

const { fullscreenLayoutIntent, mergeLayoutIntent } = require("./layout.cjs");
const { observationPane } = require("./observations.cjs");
const { stableId } = require("./utils.cjs");

function buildSurface({ observedAt, docker, adb, hosts, yggdrasilServices, verses, interfaces, observations, layout }) {
  const activeInterfaces = interfaces.filter((entry) => entry.surface?.root);
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
        ...activeInterfaces.map((entry) => ({
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
          },
          children: [entry.surface.root],
        })),
      ],
    },
    assets: [],
  };
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
  const body = sourceBody(entry.source);
  if (!body || providerId.startsWith(`${body}.`)) return providerId;
  return `${body}.${providerId}`;
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
  metric,
  pane,
  stack,
  text,
};
