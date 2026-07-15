"use strict";

class OdinLivePublicationSource {
  constructor(decode) {
    this.decode = decode;
    this.values = new Map();
    this.listeners = new Map();
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
    const entry = this.values.get(keyOf(providerId, schemaId, recordKey));
    if (!entry) throw new Error(`No accepted live publication ${schemaId}:${recordKey}.`);
    return entry.value;
  }

  watch(providerId, schemaId, recordKey, callback) {
    return this.watchLifecycle(providerId, schemaId, recordKey, event => {
      if (event.kind !== "withdrawn") callback(event.value);
    });
  }

  watchLifecycle(providerId, schemaId, recordKey, callback) {
    const key = keyOf(providerId, schemaId, recordKey);
    let listeners = this.listeners.get(key);
    if (!listeners) this.listeners.set(key, listeners = new Set());
    listeners.add(callback);
    const current = this.values.get(key);
    if (current) callback({ kind: "snapshot", sequence: current.sequence, value: current.value });
    return () => {
      listeners.delete(callback);
      if (listeners.size === 0) this.listeners.delete(key);
    };
  }

  accept(identity, document) {
    const key = keyOf(identity.providerId, document.schemaId, document.recordKey);
    const value = this.decode(document.payload);
    const previous = this.values.get(key);
    if (previous && equivalent(previous.value, value)) return;
    const entry = { sequence: ++this.sequence, value };
    this.values.set(key, entry);
    this.emit(key, { kind: previous ? "update" : "snapshot", ...entry });
  }

  withdraw(identity, document, reason = "withdrawn") {
    const key = keyOf(identity.providerId, document.schemaId, document.recordKey);
    if (!this.values.delete(key)) return;
    this.emit(key, { kind: "withdrawn", sequence: ++this.sequence, reason });
  }

  emit(key, event) {
    for (const listener of this.listeners.get(key) ?? []) listener(event);
  }
}

function keyOf(providerId, schemaId, recordKey) {
  if (!String(providerId || "").trim() || !String(schemaId || "").trim() || !String(recordKey || "").trim()) {
    throw new Error("Live publication source requires providerId, schemaId, and recordKey.");
  }
  return `${providerId}\u001f${schemaId}\u001f${recordKey}`;
}

function equivalent(left, right) {
  return JSON.stringify(left) === JSON.stringify(right);
}

module.exports = { OdinLivePublicationSource };
