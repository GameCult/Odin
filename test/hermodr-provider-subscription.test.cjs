"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");

const { ProviderSubscriptionSource } = require("../src/hermodr-provider-subscription.cjs");

function harness(reads) {
  const state = { value: reads.shift(), calls: 0, cleared: 0 };
  let tick = null;
  const source = new ProviderSubscriptionSource(
    async () => {
      state.calls += 1;
      if (state.value instanceof Error) throw state.value;
      return state.value;
    },
    {
      setInterval: (fn) => { tick = fn; return { unref() {} }; },
      clearInterval: () => { state.cleared += 1; tick = null; },
      onError: () => {},
    },
  );
  return {
    source,
    state,
    set: (value) => { state.value = value; },
    poll: async () => { if (tick) tick(); await flush(); },
    hasTimer: () => tick !== null,
  };
}

async function flush() {
  for (let i = 0; i < 4; i += 1) await new Promise((resolve) => setImmediate(resolve));
}

function collect(source, providerId = "erycina", schemaId = "gamecult.eve.surface_state.v1", recordKey = "erycina") {
  const events = [];
  const release = source.forProvider(providerId).watchLifecycle(schemaId, recordKey, (event) => events.push(event));
  return { events, release };
}

test("first watch emits a snapshot of the provider's document", async () => {
  const h = harness([{ title: "Erycina" }]);
  const { events } = collect(h.source);
  await flush();

  assert.equal(events.length, 1);
  assert.equal(events[0].kind, "snapshot");
  assert.deepEqual(events[0].value, { title: "Erycina" });
});

test("an unchanged document emits nothing further", async () => {
  const h = harness([{ title: "Erycina" }]);
  const { events } = collect(h.source);
  await flush();
  await h.poll();
  await h.poll();

  assert.equal(events.length, 1);
});

test("a changed document emits an update", async () => {
  const h = harness([{ version: 1 }]);
  const { events } = collect(h.source);
  await flush();
  h.set({ version: 2 });
  await h.poll();

  assert.equal(events.length, 2);
  assert.equal(events[1].kind, "update");
  assert.deepEqual(events[1].value, { version: 2 });
  assert.ok(events[1].sequence > events[0].sequence);
});

test("a document that disappears is withdrawn", async () => {
  const h = harness([{ version: 1 }]);
  const { events } = collect(h.source);
  await flush();
  h.set(null);
  await h.poll();

  assert.equal(events.at(-1).kind, "withdrawn");
  assert.equal(events.at(-1).reason, "withdrawn");
});

test("a document that returns after withdrawal emits a fresh snapshot", async () => {
  const h = harness([{ version: 1 }]);
  const { events } = collect(h.source);
  await flush();
  h.set(null);
  await h.poll();
  h.set({ version: 3 });
  await h.poll();

  assert.equal(events.at(-1).kind, "snapshot");
  assert.deepEqual(events.at(-1).value, { version: 3 });
});

test("a read failure is not a withdrawal", async () => {
  // A provider restart or a dropped packet must not look like the provider
  // retracting state it still holds. This is the invariant that stops a
  // transport hiccup from being rendered as authoritative absence.
  const h = harness([{ version: 1 }]);
  const { events } = collect(h.source);
  await flush();
  const before = events.length;

  h.set(new Error("rudp timeout"));
  await h.poll();
  await h.poll();

  assert.equal(events.length, before);
  assert.equal(events.at(-1).kind, "snapshot");
});

test("state survives the failure and updates once reads recover", async () => {
  const h = harness([{ version: 1 }]);
  const { events } = collect(h.source);
  await flush();
  h.set(new Error("rudp timeout"));
  await h.poll();
  h.set({ version: 9 });
  await h.poll();

  assert.equal(events.at(-1).kind, "update");
  assert.deepEqual(events.at(-1).value, { version: 9 });
});

test("releasing the last listener stops polling", async () => {
  const h = harness([{ version: 1 }]);
  const { release } = collect(h.source);
  await flush();
  assert.ok(h.hasTimer());

  release();

  assert.equal(h.state.cleared, 1);
  assert.equal(h.hasTimer(), false);
});

test("a second listener shares one poll and receives the current snapshot", async () => {
  const h = harness([{ version: 1 }]);
  const first = collect(h.source);
  await flush();
  const callsAfterFirst = h.state.calls;

  const second = collect(h.source);
  await flush();

  assert.equal(second.events[0].kind, "snapshot");
  assert.deepEqual(second.events[0].value, { version: 1 });
  // The shared entry polls once per interval regardless of listener count.
  await h.poll();
  assert.ok(h.state.calls <= callsAfterFirst + 2);

  first.release();
  assert.equal(h.state.cleared, 0, "the entry survives while one listener remains");
  second.release();
  assert.equal(h.state.cleared, 1);
});

test("watch skips withdrawal events and yields values only", async () => {
  const h = harness([{ version: 1 }]);
  const values = [];
  h.source.forProvider("erycina").watch("s", "k", (value) => values.push(value));
  await flush();
  h.set(null);
  await h.poll();
  h.set({ version: 2 });
  await h.poll();

  assert.deepEqual(values, [{ version: 1 }, { version: 2 }]);
});

test("latest refuses to invent a document the provider does not publish", async () => {
  const h = harness([null]);
  await assert.rejects(
    () => h.source.forProvider("erycina").latest("gamecult.eve.surface_state.v1", "erycina"),
    /published no/,
  );
});

test("a subscription requires provider, schema, and record key", () => {
  const h = harness([{}]);
  assert.throws(() => h.source.forProvider("").watchLifecycle("s", "k", () => {}), /requires providerId/);
  assert.throws(() => h.source.forProvider("p").watchLifecycle("", "k", () => {}), /requires providerId/);
  assert.throws(() => h.source.forProvider("p").watchLifecycle("s", "", () => {}), /requires providerId/);
});
