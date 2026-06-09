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
  const preferredHeight = positiveNumber(intent.preferredHeight, entry.providerId === "voidbot.swarm" ? 18 : 12);
  const minHeight = positiveNumber(intent.minHeight, 6);
  const preferredWidth = positiveNumber(intent.preferredWidth, 72);
  const minWidth = positiveNumber(intent.minWidth, 36);
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
    density: intent.density || "dense",
    viewportMode: intent.viewportMode || "nested-scroll",
    signalWeight: intent.signalWeight || 1,
    tree: intent.tree || null,
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
  const denseTree = isDenseLayoutStrategy(intent.layoutStrategy);
  const minWidth = positiveNumber(intent.minWidth, positiveNumber(base.minWidth, fallback.minWidth));
  const minHeight = positiveNumber(intent.minHeight, positiveNumber(base.minHeight, fallback.minHeight));
  const preferredWidth = positiveNumber(intent.preferredWidth, positiveNumber(base.preferredWidth, fallback.preferredWidth));
  const preferredHeight = positiveNumber(intent.preferredHeight, positiveNumber(base.preferredHeight, fallback.preferredHeight));
  const desiredW = Math.max(1, Math.min(4, Math.ceil(preferredWidth / 72)));
  const desiredH = Math.max(1, Math.min(4, Math.ceil(preferredHeight / 12)));
  return {
    ...base,
    minWidth,
    minHeight,
    preferredWidth,
    preferredHeight,
    h: denseTree && intent.viewportMode !== "fullscreen" ? desiredH : Math.max(positiveNumber(base.h, fallback.h), desiredH),
    w: denseTree && intent.viewportMode !== "fullscreen" ? desiredW : Math.max(positiveNumber(base.w, fallback.w), desiredW),
    priority: Number.isFinite(intent.priority) ? Math.min(Number(base.priority ?? intent.priority), intent.priority) : base.priority,
    density: intent.density || base.density || fallback.density,
    viewportMode: intent.viewportMode || base.viewportMode || fallback.viewportMode,
    layoutStrategy: intent.layoutStrategy || base.layoutStrategy || fallback.layoutStrategy,
    signalWeight: intent.signalWeight || base.signalWeight || fallback.signalWeight || 1,
    tree: intent.tree || base.tree || fallback.tree || null,
  };
}

function surfaceLayoutIntent(entry) {
  const root = entry?.surface?.root;
  if (!root || typeof root !== "object") return {};
  const layout = root.layout && typeof root.layout === "object" ? root.layout : {};
  const props = root.props && typeof root.props === "object" ? root.props : {};
  const propLayout = props.layout && typeof props.layout === "object" ? props.layout : {};
  const tree = analyzeElementTree(root);
  const derived = denseTreeLayoutIntent(tree);
  return normalizeDenseSurfaceIntent({ ...derived, ...propLayout, ...layout, tree }, derived);
}

function analyzeElementTree(root) {
  const stats = {
    elementCount: 0,
    leafCount: 0,
    branchCount: 0,
    maxDepth: 0,
    textCells: 0,
    listLikeBranchCount: 0,
    kindCounts: {},
  };

  visit(root, 1);
  stats.signalWeight = Math.max(1, stats.branchCount * 2 + stats.leafCount + Math.ceil(stats.textCells / 48));
  return stats;

  function visit(element, depth) {
    if (!element || typeof element !== "object") return;
    stats.elementCount += 1;
    stats.maxDepth = Math.max(stats.maxDepth, depth);
    const kind = String(element.kind || "unknown");
    stats.kindCounts[kind] = (stats.kindCounts[kind] || 0) + 1;
    const text = textValue(element);
    stats.textCells += text.length + Math.max(0, text.split(/\r?\n/).length - 1) * 8;
    const children = Array.isArray(element.children) ? element.children.filter((child) => child && typeof child === "object") : [];
    if (!children.length) {
      stats.leafCount += 1;
      return;
    }
    stats.branchCount += 1;
    if (children.length >= 4 && children.every((child) => String(child.kind || "") === "text" && !(Array.isArray(child.children) && child.children.length))) {
      stats.listLikeBranchCount += 1;
    }
    for (const child of children) visit(child, depth + 1);
  }
}

function denseTreeLayoutIntent(tree) {
  const signal = Math.max(1, tree.signalWeight || 1);
  const depthPressure = Math.max(0, tree.maxDepth - 2);
  const textPressure = Math.sqrt(Math.max(1, tree.textCells) / 18);
  const branchPressure = Math.sqrt(Math.max(1, tree.branchCount));
  return {
    minWidth: Math.min(72, 28 + depthPressure * 7),
    minHeight: Math.min(18, 5 + depthPressure * 2),
    preferredWidth: Math.min(144, 44 + Math.sqrt(signal) * 7 + depthPressure * 5),
    preferredHeight: Math.min(36, 8 + textPressure + branchPressure * 3 + tree.listLikeBranchCount * 2),
    density: "dense",
    viewportMode: "nested-scroll",
    layoutStrategy: "dense-tree",
    signalWeight: signal,
  };
}

function normalizeDenseSurfaceIntent(intent, derived) {
  const viewportMode = intent.viewportMode || derived.viewportMode;
  if (viewportMode === "fullscreen") {
    return intent;
  }
  return {
    ...intent,
    minWidth: Math.min(72, positiveNumber(intent.minWidth, derived.minWidth)),
    minHeight: Math.min(18, positiveNumber(intent.minHeight, derived.minHeight)),
    preferredWidth: Math.min(144, positiveNumber(intent.preferredWidth, derived.preferredWidth)),
    preferredHeight: Math.min(36, positiveNumber(intent.preferredHeight, derived.preferredHeight)),
    density: intent.density || derived.density || "dense",
    viewportMode,
    layoutStrategy: intent.layoutStrategy || derived.layoutStrategy || "dense-tree",
    signalWeight: intent.signalWeight || derived.signalWeight || 1,
  };
}

function isDenseLayoutStrategy(strategy) {
  const value = String(strategy || "");
  return value === "dense-tree" || value === "legacy-node-groups" || value.startsWith("dense-");
}

function textValue(element) {
  if (typeof element.text === "string") return element.text;
  const props = element.props && typeof element.props === "object" ? element.props : {};
  if (typeof props.text === "string") return props.text;
  if (typeof props.title === "string") return props.title;
  return "";
}

module.exports = {
  analyzeElementTree,
  createLayoutStore,
  defaultLayoutFor,
  denseTreeLayoutIntent,
  fullscreenLayoutIntent,
  mergeLayoutIntent,
  surfaceLayoutIntent,
};
