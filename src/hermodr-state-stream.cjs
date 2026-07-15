"use strict";

class HermodrStateStreamRegistry {
  constructor(source) { this.source = source; this.entries = new Map(); }
  acquire(selection, listener) {
    const key = `${selection.schemaId}\u001f${selection.recordKey}`;
    let entry = this.entries.get(key);
    if (!entry) {
      entry = { listeners: new Set([listener]), sequence: 0, stale: false, unsubscribe: null };
      entry.unsubscribe = this.source.forProvider(selection.providerId).watchLifecycle(selection.schemaId, selection.recordKey, event => {
        const type = event.kind === "withdrawn" ? "stale" : entry.stale ? "reconnected" : event.kind;
        entry.stale = type === "stale";
        const lowered = { type, sequence: ++entry.sequence, source: selection,
          ...(event.value === undefined ? {} : { value: event.value }),
          ...(event.reason ? { reason: event.reason } : {}) };
        for (const subscriber of entry.listeners) subscriber(lowered);
      });
      this.entries.set(key, entry);
    } else {
      entry.listeners.add(listener);
    }
    let released = false;
    return () => {
      if (released) return;
      released = true;
      entry.listeners.delete(listener);
      if (entry.listeners.size === 0) { entry.unsubscribe(); this.entries.delete(key); }
    };
  }
}

module.exports = { HermodrStateStreamRegistry };
