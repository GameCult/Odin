"use strict";

const fs = require("fs");
const {
  CultCache,
  SingleFileMessagePackBackingStore,
  defineDocumentRegistry,
  defineDocumentType,
} = require("cultcache-ts");
const { httpGet } = require("./utils.cjs");

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

async function buildMarqueeText({ interfaces, textDocumentStorePath, stonksBurstSize = 8, stonksStateUrl }) {
  const [securities, stanzas] = await Promise.all([
    stonksSegments({ interfaces, stonksStateUrl }),
    poetryStanzas(textDocumentStorePath),
  ]);
  return stanzaBurstTape(stanzas, securities, stonksBurstSize).join(" / ");
}

async function stonksSegments({ interfaces, stonksStateUrl }) {
  const fromInterface = stonksSegmentsFromInterfaces(interfaces);
  if (fromInterface.length) return fromInterface;
  return stonksSegmentsFromSnapshot(stonksStateUrl);
}

function stonksSegmentsFromInterfaces(interfaces) {
  const stonks = interfaces.find((entry) => String(entry.providerId || "").toLowerCase() === "stonks.market");
  const explicit = String(stonks?.surface?.root?.props?.marqueeText || "").trim();
  return splitSegments(explicit).slice(0, 16);
}

async function stonksSegmentsFromSnapshot(stonksStateUrl) {
  if (!stonksStateUrl) return [];
  try {
    const snapshot = JSON.parse(await httpGet(stonksStateUrl, 1200));
    const quotes = [...array(snapshot.crypto), ...array(snapshot.equities)]
      .slice(0, 16)
      .map(quoteText)
      .filter(Boolean);
    const warnings = [
      snapshot?.sources?.equities?.ok === false ? "EQUITIES SOURCE ERROR" : "",
      snapshot?.sources?.crypto?.ok === false ? "CRYPTO SOURCE ERROR" : "",
    ].filter(Boolean);
    return [...quotes, ...warnings];
  } catch {
    return [];
  }
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

function quoteText(item) {
  if (!item?.symbol) return "";
  const price = item.price == null ? "N/D" : `$${formatNumber(item.price)}`;
  const change = item.change24h == null ? "" : ` ${item.change24h >= 0 ? "+" : ""}${Number(item.change24h).toFixed(2)}%`;
  const volume = item.volume24h ?? item.volume;
  const volumeText = volume == null ? "" : ` vol ${formatCompact(volume)}`;
  return `${item.symbol} ${price}${change}${volumeText}`;
}

function formatNumber(value) {
  const number = Number(value);
  if (!Number.isFinite(number)) return "N/D";
  if (Math.abs(number) >= 1000) return number.toFixed(0);
  if (Math.abs(number) >= 1) return number.toFixed(2);
  return number.toFixed(4);
}

function formatCompact(value) {
  const number = Number(value);
  if (!Number.isFinite(number)) return "N/D";
  return Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 1 }).format(number);
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
