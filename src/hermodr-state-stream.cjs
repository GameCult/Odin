"use strict";

const { EventEmitter } = require("node:events");

class HermodrStateStream extends EventEmitter {
  constructor(source, read, options = {}) {
    super();
    this.source = Object.freeze({ ...source });
    this.read = read;
    this.pollIntervalMs = options.pollIntervalMs ?? 1_000;
    this.staleAfterMs = options.staleAfterMs ?? 10_000;
    this.schedule = options.schedule ?? ((callback, delay) => setTimeout(callback, delay));
    this.cancel = options.cancel ?? clearTimeout;
    this.now = options.now ?? Date.now;
    this.sequence = 0;
    this.lastValue = undefined;
    this.lastFingerprint = "";
    this.lastSuccessAt = 0;
    this.stale = false;
    this.running = false;
    this.timer = null;
  }

  start() {
    if (this.running) return;
    this.running = true;
    void this.poll();
  }

  stop() {
    this.running = false;
    if (this.timer !== null) this.cancel(this.timer);
    this.timer = null;
  }

  async poll() {
    if (!this.running) return;
    try {
      const value = await this.read();
      const fingerprint = stableFingerprint(value);
      const recovered = this.stale;
      const changed = fingerprint !== this.lastFingerprint;
      this.lastSuccessAt = this.now();
      this.stale = false;
      if (changed || recovered) {
        this.lastValue = value;
        this.lastFingerprint = fingerprint;
        this.publish(recovered ? "reconnected" : this.sequence === 0 ? "snapshot" : "update", value);
      }
    } catch (error) {
      const elapsed = this.lastSuccessAt ? this.now() - this.lastSuccessAt : this.staleAfterMs;
      if (!this.stale && elapsed >= this.staleAfterMs) {
        this.stale = true;
        this.publish("stale", undefined, error);
      }
    } finally {
      if (this.running) this.timer = this.schedule(() => void this.poll(), this.pollIntervalMs);
    }
  }

  publish(type, value, error) {
    const event = {
      type,
      sequence: ++this.sequence,
      observedAt: new Date(this.now()).toISOString(),
      source: this.source,
      ...(value === undefined ? {} : { value }),
      ...(error ? { error: error instanceof Error ? error.message : String(error) } : {}),
    };
    this.emit("event", event);
  }
}

class HermodrStateStreamRegistry {
  constructor(options = {}) {
    this.options = options;
    this.streams = new Map();
  }

  acquire(source, read, listener) {
    const key = stateSourceKey(source);
    let entry = this.streams.get(key);
    if (!entry) {
      const stream = new HermodrStateStream(source, read, this.options);
      entry = { stream, subscribers: 0 };
      this.streams.set(key, entry);
    }
    entry.subscribers += 1;
    entry.stream.on("event", listener);
    entry.stream.start();
    let released = false;
    return () => {
      if (released) return;
      released = true;
      entry.stream.off("event", listener);
      entry.subscribers -= 1;
      if (entry.subscribers === 0) {
        entry.stream.stop();
        this.streams.delete(key);
      }
    };
  }
}

function stateSourceKey(source) {
  return [source.providerId, source.schemaId, source.documentId].map(value => String(value || "")).join("\u001f");
}

function stableFingerprint(value) {
  return JSON.stringify(value, (_key, item) => {
    if (!item || typeof item !== "object" || Array.isArray(item)) return item;
    return Object.fromEntries(Object.entries(item).sort(([left], [right]) => left.localeCompare(right)));
  });
}

module.exports = { HermodrStateStream, HermodrStateStreamRegistry, stateSourceKey };
