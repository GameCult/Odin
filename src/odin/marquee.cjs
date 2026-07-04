"use strict";

const fs = require("fs");
const {
  CultCache,
  SingleFileMessagePackBackingStore,
  defineDocumentRegistry,
  defineDocumentType,
} = require("cultcache-ts");

const textDocumentSetDefinition = defineDocumentType({
  type: "gamecult.text_document_set",
  global: true,
  schema: {
    parse(value) {
      if (!value || typeof value !== "object") {
        throw new Error("gamecult.text_document_set must be an object");
      }
      return value;
    },
  },
});

async function buildMarqueeText({ interfaces, textDocumentStorePath, stonksBurstSize = 8 }) {
  const [securities, stanzas] = await Promise.all([
    Promise.resolve(stonksSegments({ interfaces })),
    poetryStanzas(textDocumentStorePath),
  ]);
  return stanzaBurstTape(stanzas, securities, stonksBurstSize).join(" / ");
}

function stonksSegments({ interfaces }) {
  return stonksSegmentsFromInterfaces(interfaces);
}

function stonksSegmentsFromInterfaces(interfaces) {
  const stonks = interfaces.find((entry) => String(entry.providerId || "").toLowerCase() === "stonks.market");
  const explicit = String(stonks?.surface?.root?.props?.marqueeText || "").trim();
  return splitSegments(explicit).slice(0, 16);
}

async function poetryStanzas(textDocumentStorePath) {
  if (!textDocumentStorePath || !fs.existsSync(textDocumentStorePath)) return [];
  try {
    const cache = CultCache.builder()
      .withRegistry(defineDocumentRegistry(textDocumentSetDefinition))
      .withGenericStore(new SingleFileMessagePackBackingStore(textDocumentStorePath))
      .build();
    await cache.pullAllBackingStores();
    const documentSet = cache.getGlobal(textDocumentSetDefinition);
    return array(documentSet?.documents).flatMap(documentStanzas);
  } catch {
    return [];
  }
}

function documentStanzas(document) {
  const stanzas = [];
  let stanza = [];
  for (const line of array(document?.lines)) {
    if (isStanzaMarker(line)) {
      pushStanza(stanzas, stanza);
      stanza = [];
      continue;
    }

    const text = array(line).join(" / ").replace(/\s+/g, " ").trim();
    if (text) stanza.push(text);
  }
  pushStanza(stanzas, stanza);
  return stanzas;
}

function pushStanza(stanzas, stanza) {
  if (stanza.length) stanzas.push(stanza.join(" / "));
}

function stanzaBurstTape(stanzas, securities, burstSize) {
  const output = [];
  const stonkBurstSize = Math.max(1, Number(burstSize) || 8);
  const stanzaList = stanzas.length ? stanzas : [];
  const securityList = securities.length ? securities : [];
  const cycles = Math.max(stanzaList.length, securityList.length ? Math.ceil(securityList.length / stonkBurstSize) : 0);
  for (let index = 0; index < cycles; index += 1) {
    if (stanzaList.length) output.push(stanzaList[index % stanzaList.length]);
    for (let offset = 0; offset < stonkBurstSize && securityList.length; offset += 1) {
      output.push(securityList[(index * stonkBurstSize + offset) % securityList.length]);
    }
  }
  return output;
}

function splitSegments(text) {
  return String(text || "")
    .split(/\s+\/\s+/)
    .map((segment) => segment.replace(/\s+/g, " ").trim())
    .filter(Boolean);
}

function isStanzaMarker(line) {
  const parts = array(line);
  return parts.length === 1 && String(parts[0]).trim().toLowerCase() === "[stanza]";
}

function array(value) {
  return Array.isArray(value) ? value : [];
}

module.exports = {
  buildMarqueeText,
};
