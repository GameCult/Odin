"use strict";

const fs = require("fs");
const { stableId, tailTextFile } = require("./utils.cjs");

async function observationSnapshot(source, freshSeconds) {
  if (!fs.existsSync(source)) {
    return {
      state: "missing",
      source,
      detail: `missing ${source}`,
      streams: [],
    };
  }

  try {
    const lines = tailTextFile(source, 512 * 1024)
      .split(/\r?\n/)
      .filter(Boolean);
    const nowMs = Date.now();
    const byStream = new Map();
    let accepted = 0;
    for (const line of lines) {
      let item;
      try {
        item = JSON.parse(line);
      } catch {
        continue;
      }

      if (item.type !== "cultmesh-observation" && item.type !== "cultmesh-media-observation") {
        continue;
      }

      accepted += 1;
      const key = `${item.DeviceId || "unknown"}:${item.StreamId || "unknown"}:${item.Kind || "unknown"}`;
      byStream.set(key, item);
    }

    const streams = [...byStream.values()]
      .map((item) => observationStream(item, nowMs, freshSeconds))
      .sort((left, right) => left.deviceId.localeCompare(right.deviceId)
        || left.streamId.localeCompare(right.streamId)
        || left.kind.localeCompare(right.kind));

    const activeCount = streams.filter((entry) => entry.state === "active").length;
    return {
      state: streams.length ? (activeCount ? "active" : "stale") : "waiting",
      source,
      detail: `${streams.length} streams, ${activeCount} active, ${accepted} recent records`,
      streams,
    };
  } catch (error) {
    return {
      state: "error",
      source,
      detail: error.message,
      streams: [],
    };
  }
}

function observationStream(item, nowMs, freshSeconds) {
  const latestMs = Date.parse(item.WallClockUtc || "");
  const ageSeconds = Number.isFinite(latestMs) ? Math.max(0, Math.round((nowMs - latestMs) / 1000)) : null;
  return {
    deviceId: String(item.DeviceId || "unknown"),
    streamId: String(item.StreamId || "unknown"),
    kind: String(item.Kind || "unknown"),
    document: String(item.document || "unknown"),
    state: ageSeconds === null ? "unknown" : ageSeconds <= freshSeconds ? "active" : "stale",
    sequence: item.Sequence ?? null,
    latestAt: item.WallClockUtc || "",
    ageSeconds,
    clockDomainId: item.ClockDomainId || "",
    format: item.Format || "",
    width: item.Width ?? null,
    height: item.Height ?? null,
    sampleRate: item.SampleRate ?? null,
    channels: item.Channels ?? null,
    frameCount: item.FrameCount ?? null,
    payloadEncoding: item.PayloadEncoding || "",
    payloadBytes: item.PayloadBytes ?? null,
    values: Array.isArray(item.Values) ? item.Values : null,
    accuracy: item.Accuracy ?? null,
    action: item.Action ?? null,
    pointerCount: item.PointerCount ?? null,
    x: item.X ?? null,
    y: item.Y ?? null,
  };
}

function observationServices(observations, deviceId, service) {
  return observations.streams
    .filter((entry) => entry.deviceId === deviceId)
    .map((entry) => service(
      `observation-${entry.streamId}-${entry.kind}`,
      `${entry.kind} observation`,
      entry.state,
      observationShape(entry),
    ));
}

function observationPane(observations, text) {
  return {
    id: "observation-streams",
    kind: "pane",
    props: {
      title: "Device Observation Streams",
      source: observations.source,
      status: observations.state,
      detail: observations.detail,
    },
    children: observations.streams.length
      ? observations.streams.map((entry) => observationStreamNode(entry, text))
      : [text("observation-streams-empty", observations.detail || "no observation streams")],
  };
}

function observationStreamNode(entry, text) {
  return {
    id: `observation-stream-${stableId(entry.deviceId)}-${stableId(entry.streamId)}-${stableId(entry.kind)}`,
    kind: "observation-stream",
    props: entry,
    children: [
      text(`observation-${stableId(entry.streamId)}-schema`, entry.document),
      text(`observation-${stableId(entry.streamId)}-latest`, `${entry.kind} seq ${entry.sequence} age ${entry.ageSeconds}s`),
      text(`observation-${stableId(entry.streamId)}-shape`, observationShape(entry)),
    ],
  };
}

function observationShape(entry) {
  if (entry.width && entry.height) {
    return `${entry.format || "media"} ${entry.width}x${entry.height}, ${entry.payloadBytes || 0} bytes`;
  }

  if (entry.sampleRate) {
    return `${entry.format || "audio"} ${entry.sampleRate} Hz x ${entry.channels || 1}, ${entry.frameCount || 0} frames, ${entry.payloadBytes || 0} bytes`;
  }

  if (entry.values) {
    return `${entry.values.map((value) => Number(value).toFixed(3)).join(", ")} accuracy ${entry.accuracy ?? "unknown"}`;
  }

  if (entry.action) {
    return `${entry.action} pointers ${entry.pointerCount ?? "unknown"} @ ${entry.x ?? "?"},${entry.y ?? "?"}`;
  }

  return entry.state;
}

module.exports = {
  observationPane,
  observationServices,
  observationShape,
  observationSnapshot,
};
