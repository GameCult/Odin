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
        summary: `${verses.length} Verses / ${verses.reduce((sum, entry) => sum + entry.services.length, 0)} services / ${activeInterfaces.length} interfaces / ${activeObservationStreams.length} live streams`,
        layout: fullscreenLayoutIntent("odin.allseer", -100),
      },
      children: [
        pane("Coordinator", [
          text("observed", `observed ${observedAt}`),
          text("authority", "Odin owns Verse discovery, schema awareness, translation planning, and accepted surface publication. Renderers lower only."),
          metric("docker-count", "Docker containers", docker.containers.length, docker.state === "ok" ? "ok" : "warn"),
          metric("adb-count", "ADB devices", adb.devices.length, adb.devices.length ? "ok" : "warn"),
          metric("observation-stream-count", "Observation streams", observations.streams.length, activeObservationStreams.length ? "ok" : "warn"),
          text("observation-ledger", `observation ledger: ${observations.detail}`),
        ]),
        observationPane(observations, text),
        ...interfaces.map((entry) => ({
          id: `interface-${stableId(entry.providerId)}`,
          kind: "interface",
          props: {
            title: entry.title,
            providerId: entry.providerId,
            source: entry.source,
            status: entry.state,
            detail: entry.detail,
            version: entry.version,
            updatedAt: entry.updatedAt,
            layout: mergeLayoutIntent(layout.tiles?.[entry.providerId], entry, interfaces),
          },
          children: entry.surface?.root ? [entry.surface.root] : [
            text(`interface-${stableId(entry.providerId)}-missing`, entry.detail || "interface unavailable"),
          ],
        })),
        ...verses.map((entry) => ({
          id: `verse-${entry.verseId}`,
          kind: "verse",
          props: {
            title: entry.name,
            verseId: entry.verseId,
            role: entry.role,
            status: entry.status,
            capabilities: entry.capabilities,
            services: entry.services,
          },
          children: entry.services.map((item) => ({
            id: `service-${entry.verseId}-${item.id}`,
            kind: "service",
            props: item,
            children: [],
          })),
        })),
        pane("Verses", verses.map((entry) =>
          card(`verse-card-${entry.verseId}`, [
            text(`verse-title-${entry.verseId}`, `${entry.name} :: ${entry.verseId}`),
            text(`verse-status-${entry.verseId}`, `${entry.role} / ${entry.status}`),
            text(`verse-caps-${entry.verseId}`, entry.capabilities.join(", ")),
          ], entry.status),
        )),
        pane("Hosts", Object.entries(hosts).map(([name, checks]) =>
          card(`host-${name}`, [
            text(`host-name-${name}`, name),
            ...checks.map((check) => text(`host-${name}-${check.name}`, `${check.name}: ${check.state}${check.detail ? ` ${check.detail}` : ""}`)),
          ], checks.some((check) => check.state === "open" || check.state === "ok") ? "ok" : "warn"),
        )),
        pane("Yggdrasil Services", yggdrasilServices.map((service) =>
          text(`ygg-${service.name}`, `${service.name}: ${service.state}`),
        )),
        pane("Docker", docker.containers.length
          ? docker.containers.slice(0, 10).map((container) => text(`docker-${container.name}`, `${container.name}: ${container.status} (${container.image})`))
          : [text("docker-empty", docker.error || "no running containers")]),
        pane("Periwinkle / ADB", adb.devices.length
          ? adb.devices.map((device) => text(`adb-${device.serial}`, `${device.serial}: ${device.state}`))
          : [text("adb-empty", adb.error || "no devices")]),
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

module.exports = {
  buildPendingSurface,
  buildSurface,
  card,
  metric,
  pane,
  stack,
  text,
};
