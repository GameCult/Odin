"use strict";

const fs = require("fs");
const { clampNumber, positiveNumber, stableId } = require("./utils.cjs");

function createLayoutStore(layoutPath) {
  return {
    readLayout() {
      try {
        return JSON.parse(fs.readFileSync(layoutPath, "utf8"));
      } catch {
        return { schema: "odin.interface_layout.v1", tiles: {} };
      }
    },

    writeLayout(layout) {
      fs.writeFileSync(layoutPath, JSON.stringify(layout, null, 2), "utf8");
    },

    applyClientCommand(command) {
      if (!command || command.type !== "odin-layout-intent" || !command.providerId) return;
      const layout = this.readLayout();
      layout.schema = "odin.interface_layout.v1";
      layout.updatedAt = new Date().toISOString();
      layout.tiles ||= {};
      const current = layout.tiles[command.providerId] || defaultLayoutFor({ providerId: command.providerId }, []);
      const next = { ...current };
      if (command.action === "focus") {
        Object.assign(next, fullscreenLayoutIntent(command.providerId, -100));
      } else if (command.action === "resize") {
        next.w = clampNumber((next.w || 1) + Number(command.dw || 0), 1, 4);
        next.h = clampNumber((next.h || 1) + Number(command.dh || 0), 1, 4);
      } else if (command.action === "move") {
        next.x = clampNumber((next.x || 0) + Number(command.dx || 0), 0, 12);
        next.y = clampNumber((next.y || 0) + Number(command.dy || 0), 0, 12);
      } else if (command.action === "toggle") {
        next.visible = !next.visible;
      } else {
        return;
      }
      layout.tiles[command.providerId] = next;
      this.writeLayout(layout);
    },
  };
}

function defaultLayoutFor(entry, interfaces) {
  const index = Math.max(0, interfaces.findIndex((candidate) => candidate.providerId === entry.providerId));
  const intent = surfaceLayoutIntent(entry);
  const preferredHeight = positiveNumber(intent.preferredHeight, entry.providerId === "voidbot.swarm" ? 16 : 10);
  const minHeight = positiveNumber(intent.minHeight, 8);
  const preferredWidth = positiveNumber(intent.preferredWidth, 96);
  const minWidth = positiveNumber(intent.minWidth, 48);
  const priority = Number.isFinite(intent.priority) ? intent.priority : index;
  return {
    tileId: stableId(entry.providerId || "interface"),
    visible: true,
    priority,
    x: index % 2,
    y: Math.floor(index / 2),
    w: 1,
    h: Math.max(1, Math.ceil(preferredHeight / 12)),
    minWidth,
    minHeight,
    preferredWidth,
    preferredHeight,
    density: intent.density || "adaptive",
    viewportMode: intent.viewportMode || "adaptive",
  };
}

function fullscreenLayoutIntent(tileId, priority = -100) {
  return {
    tileId: stableId(tileId || "fullscreen"),
    visible: true,
    priority,
    x: 0,
    y: 0,
    w: 4,
    h: 4,
    minWidth: 96,
    minHeight: 36,
    preferredWidth: 192,
    preferredHeight: 48,
    density: "comfortable",
    viewportMode: "fullscreen",
  };
}

function mergeLayoutIntent(existing, entry, interfaces) {
  const base = existing && typeof existing === "object"
    ? { ...existing }
    : defaultLayoutFor(entry, interfaces);
  const intent = surfaceLayoutIntent(entry);
  const fallback = defaultLayoutFor(entry, interfaces);
  const minWidth = positiveNumber(intent.minWidth, positiveNumber(base.minWidth, fallback.minWidth));
  const minHeight = positiveNumber(intent.minHeight, positiveNumber(base.minHeight, fallback.minHeight));
  const preferredWidth = positiveNumber(intent.preferredWidth, positiveNumber(base.preferredWidth, fallback.preferredWidth));
  const preferredHeight = positiveNumber(intent.preferredHeight, positiveNumber(base.preferredHeight, fallback.preferredHeight));
  return {
    ...base,
    minWidth,
    minHeight,
    preferredWidth: Math.max(positiveNumber(base.preferredWidth, preferredWidth), preferredWidth),
    preferredHeight: Math.max(positiveNumber(base.preferredHeight, preferredHeight), preferredHeight),
    h: Math.max(positiveNumber(base.h, fallback.h), Math.ceil(preferredHeight / 12)),
    w: Math.max(positiveNumber(base.w, fallback.w), Math.ceil(preferredWidth / 96)),
    priority: Number.isFinite(intent.priority) ? Math.min(Number(base.priority ?? intent.priority), intent.priority) : base.priority,
    density: intent.density || base.density || fallback.density,
    viewportMode: intent.viewportMode || base.viewportMode || fallback.viewportMode,
  };
}

function surfaceLayoutIntent(entry) {
  const root = entry?.surface?.root;
  if (!root || typeof root !== "object") return {};
  const layout = root.layout && typeof root.layout === "object" ? root.layout : {};
  const props = root.props && typeof root.props === "object" ? root.props : {};
  const propLayout = props.layout && typeof props.layout === "object" ? props.layout : {};
  return { ...propLayout, ...layout };
}

module.exports = {
  createLayoutStore,
  defaultLayoutFor,
  fullscreenLayoutIntent,
  mergeLayoutIntent,
  surfaceLayoutIntent,
};
