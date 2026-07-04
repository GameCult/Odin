"use strict";

const fs = require("fs");
const path = require("path");
const { execFile } = require("child_process");
const { promisify } = require("util");

const execFileAsync = promisify(execFile);

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

module.exports = {
  adbSnapshot,
  dockerSnapshot,
};
