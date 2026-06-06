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

async function buildMarqueeText({ interfaces, textDocumentStorePath, stonksStateUrl }) {
  const [securities, poetry] = await Promise.all([
    stonksSegments({ interfaces, stonksStateUrl }),
    poetrySegments(textDocumentStorePath),
  ]);
  return interweave(securities, poetry).join(" / ");
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

async function poetrySegments(textDocumentStorePath) {
  if (!textDocumentStorePath || !fs.existsSync(textDocumentStorePath)) return [];
  try {
    const cache = CultCache.builder()
      .withRegistry(defineDocumentRegistry(textDocumentSetDefinition))
      .withGenericStore(new SingleFileMessagePackBackingStore(textDocumentStorePath))
      .build();
    await cache.pullAllBackingStores();
    const documentSet = cache.getGlobal(textDocumentSetDefinition);
    return array(documentSet?.documents)
      .flatMap((document) => array(document.lines))
      .filter((line) => !isStanzaMarker(line))
      .map((line) => array(line).join(" / ").replace(/\s+/g, " ").trim())
      .filter(Boolean);
  } catch {
    return [];
  }
}

function interweave(securities, poetry) {
  const output = [];
  const count = Math.max(securities.length, poetry.length);
  for (let index = 0; index < count; index += 1) {
    if (securities[index]) output.push(securities[index]);
    if (poetry[index]) output.push(poetry[index]);
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
