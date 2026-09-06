"use strict";

// Provider state subscription for the browser lowering.
//
// Hermodr previously ran its own provider-session ingress and accepted
// publications into a private store, which made it a required ingress rather
// than a bridge: a surface reached a browser only if its provider bound to
// Hermodr specifically. Providers own their state. Odin publishes where to find
// it. This source resolves a provider through the catalog and reads that
// provider directly, exactly as the command path already routes intent.
//
// It presents the same narrow port the previous store did -
// forProvider(id) -> { latest, watch, watchLifecycle } - so
// HermodrStateStreamRegistry, its sequence numbers, and its stale/reconnected
// transitions are untouched.
//
// readDocument is injected. That keeps the polling and lifecycle semantics
// testable without a live Verse, and keeps CultMesh transport out of this file.

const DEFAULT_POLL_INTERVAL_MS = 2_000;

class ProviderSubscriptionSource {
  constructor(readDocument, options = {}) {
    if (typeof readDocument !== "function") {
      throw new Error("Provider subscription source requires a readDocument(providerId, schemaId, recordKey) function.");
    }
    this.readDocument = readDocument;
    this.pollIntervalMs = Number.isFinite(options.pollIntervalMs) && options.pollIntervalMs > 0
      ? options.pollIntervalMs
      : DEFAULT_POLL_INTERVAL_MS;
    this.setInterval = options.setInterval || setInterval;
    this.clearInterval = options.clearInterval || clearInterval;
    this.onError = options.onError || (() => {});
    this.entries = new Map();
    this.sequence = 0;
  }

  forProvider(providerId) {
    return {
      latest: (schemaId, recordKey) => this.latest(providerId, schemaId, recordKey),
      watch: (schemaId, recordKey, callback) => this.watch(providerId, schemaId, recordKey, callback),
      watchLifecycle: (schemaId, recordKey, callback) => this.watchLifecycle(providerId, schemaId, recordKey, callback),
    };
  }

  async latest(providerId, schemaId, recordKey) {
    const value = await this.readDocument(providerId, schemaId, recordKey);
    if (value === undefined || value === null) {
      throw new Error(`Provider ${providerId} published no ${schemaId}:${recordKey}.`);
    }
    return value;
  }

  watch(providerId, schemaId, recordKey, callback) {
    return this.watchLifecycle(providerId, schemaId, recordKey, (event) => {
      if (event.kind !== "withdrawn") callback(event.value);
    });
  }

  watchLifecycle(providerId, schemaId, recordKey, callback) {
    const key = keyOf(providerId, schemaId, recordKey);
    let entry = this.entries.get(key);
    if (!entry) {
      entry = {
        providerId,
        schemaId,
        recordKey,
        listeners: new Set(),
        value: undefined,
        present: false,
        timer: null,
        polling: false,
      };
      this.entries.set(key, entry);
      entry.timer = this.setInterval(() => { this.poll(key); }, this.pollIntervalMs);
      if (typeof entry.timer?.unref === "function") entry.timer.unref();
    }

    entry.listeners.add(callback);
    if (entry.present) {
      callback({ kind: "snapshot", sequence: ++this.sequence, value: entry.value });
    }
    // First listener on a cold entry still needs a value without waiting a full
    // interval. Subsequent listeners piggyback on the shared poll.
    this.poll(key);

    let released = false;
    return () => {
      if (released) return;
      released = true;
      entry.listeners.delete(callback);
      if (entry.listeners.size === 0) {
        this.clearInterval(entry.timer);
        this.entries.delete(key);
      }
    };
  }

  async poll(key) {
    const entry = this.entries.get(key);
    if (!entry || entry.polling) return;
    entry.polling = true;
    try {
      let value;
      try {
        value = await this.readDocument(entry.providerId, entry.schemaId, entry.recordKey);
      } catch (error) {
        // A read failure is not a withdrawal. The provider may be restarting,
        // and claiming withdrawal would let a transport hiccup look like the
        // provider retracting state it still holds.
        this.onError(error, { providerId: entry.providerId, schemaId: entry.schemaId, recordKey: entry.recordKey });
        return;
      }

      if (!this.entries.has(key)) return;

      const missing = value === undefined || value === null;
      if (missing) {
        if (entry.present) {
          entry.present = false;
          entry.value = undefined;
          this.emit(entry, { kind: "withdrawn", sequence: ++this.sequence, reason: "withdrawn" });
        }
        return;
      }

      if (entry.present && equivalent(entry.value, value)) return;

      const kind = entry.present ? "update" : "snapshot";
      entry.present = true;
      entry.value = value;
      this.emit(entry, { kind, sequence: ++this.sequence, value });
    } finally {
      entry.polling = false;
    }
  }

  emit(entry, event) {
    for (const listener of entry.listeners) listener(event);
  }
}

function keyOf(providerId, schemaId, recordKey) {
  if (!String(providerId || "").trim() || !String(schemaId || "").trim() || !String(recordKey || "").trim()) {
    throw new Error("Provider subscription requires providerId, schemaId, and recordKey.");
  }
  return `${providerId}\u001f${schemaId}\u001f${recordKey}`;
}

function equivalent(left, right) {
  return JSON.stringify(left) === JSON.stringify(right);
}

module.exports = { ProviderSubscriptionSource, DEFAULT_POLL_INTERVAL_MS };
