"use strict";

const fs = require("fs");
const net = require("net");
const os = require("os");
const path = require("path");
const { execFile } = require("child_process");
const { promisify } = require("util");

const execFileAsync = promisify(execFile);

async function hostChecks() {
  return {
    Starfire: [{ name: "local", state: "ok", detail: os.hostname() }],
    Nightwing: await checksFor("192.168.1.75", [["ssh", 22], ["eve-broker", 8795], ["eve-browser", 8891]]),
    Raven: await checksFor("192.168.1.84", [["ssh", 22]]),
    EVE: await checksFor("192.168.1.72", [["ssh", 22]]),
    Yggdrasil: await checksFor("yggdrasil.gamecult.org", [["ssh", 22], ["http", 80], ["https", 443]]),
  };
}

async function checksFor(host, specs) {
  return Promise.all(specs.map(async ([name, port]) => ({
    name,
    ...(await tcpCheck(host, port)),
  })));
}

function tcpCheck(host, port) {
  return new Promise((resolve) => {
    const socket = net.createConnection({ host, port, timeout: 900 });
    socket.on("connect", () => {
      socket.destroy();
      resolve({ state: "open", detail: `${host}:${port}` });
    });
    socket.on("timeout", () => {
      socket.destroy();
      resolve({ state: "timeout", detail: `${host}:${port}` });
    });
    socket.on("error", (error) => {
      resolve({ state: "closed", detail: error.code || error.message });
    });
  });
}

async function dockerSnapshot() {
  try {
    const { stdout } = await execFileAsync("docker", ["ps", "--format", "{{json .}}"], { timeout: 3000 });
    const containers = stdout.split(/\r?\n/)
      .filter(Boolean)
      .map((line) => JSON.parse(line))
      .map((item) => ({
        name: item.Names,
        image: item.Image,
        status: item.Status,
        ports: item.Ports,
      }));
    return { state: "ok", containers, error: "" };
  } catch (error) {
    return { state: "error", containers: [], error: error.message };
  }
}

async function adbSnapshot() {
  const adb = path.join(process.env.LOCALAPPDATA || "", "Android", "Sdk", "platform-tools", "adb.exe");
  if (!fs.existsSync(adb)) {
    return { state: "missing", devices: [], error: `missing ${adb}` };
  }
  try {
    const { stdout } = await execFileAsync(adb, ["devices"], { timeout: 3000 });
    const devices = stdout.split(/\r?\n/)
      .map((line) => line.match(/^(\S+)\s+(device|offline|unauthorized)$/))
      .filter(Boolean)
      .map((match) => ({ serial: match[1], state: match[2] }));
    return { state: "ok", devices, error: "" };
  } catch (error) {
    return { state: "error", devices: [], error: error.message };
  }
}

async function remoteServices(target, services) {
  const script = `for s in ${services.join(" ")}; do printf '%s=' "$s"; systemctl is-active "$s" 2>/dev/null || true; done`;
  try {
    const { stdout } = await execFileAsync("ssh", ["-o", "BatchMode=yes", "-o", "ConnectTimeout=3", target, script], { timeout: 6000 });
    return stdout.split(/\r?\n/)
      .map((line) => line.match(/^([^=]+)=(.+)$/))
      .filter(Boolean)
      .map((match) => ({ name: match[1], state: match[2].trim() }));
  } catch (error) {
    return services.map((name) => ({ name, state: `unknown: ${error.message}` }));
  }
}

async function remoteGpu(target) {
  try {
    const { stdout } = await execFileAsync("ssh", [
      "-o", "BatchMode=yes",
      "-o", "ConnectTimeout=3",
      target,
      "nvidia-smi --query-gpu=name,driver_version,memory.total,utilization.gpu --format=csv,noheader,nounits 2>/dev/null | head -n 1",
    ], { timeout: 6000 });
    const detail = stdout.trim();
    return detail ? { state: "active", detail } : { state: "unknown", detail: "nvidia-smi returned no GPU" };
  } catch (error) {
    return { state: "unknown", detail: error.message };
  }
}

function hostState(checks) {
  if (!checks) return "unknown";
  if (checks.some((check) => check.state === "open" || check.state === "ok")) return "reachable";
  if (checks.some((check) => check.state === "timeout")) return "timeout";
  return "offline";
}

function hostServices(hostName, checks) {
  return (checks || []).map((check) =>
    service(`${hostName}-${check.name}`, `${hostName} ${check.name}`, check.state === "open" || check.state === "ok" ? "active" : check.state, check.detail),
  );
}

function systemdState(state) {
  if (state === "active") return "active";
  if (state === "inactive") return "inactive";
  if (state === "failed") return "failed";
  return state.startsWith("unknown") ? "unknown" : "warn";
}

function service(id, name, state, detail = "") {
  return { id: require("./utils.cjs").stableId(id), name, state, detail: String(detail || "") };
}

module.exports = {
  adbSnapshot,
  checksFor,
  dockerSnapshot,
  hostChecks,
  hostServices,
  hostState,
  remoteGpu,
  remoteServices,
  systemdState,
  tcpCheck,
};
