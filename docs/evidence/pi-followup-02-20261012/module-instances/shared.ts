import fs from "node:fs";
const id = Math.random().toString(36).slice(2);
let loads = 0;
export function mark(name) {
  loads += 1;
  fs.appendFileSync(process.env.PROBE_LOG, JSON.stringify({ name, module: id, loads, pid: process.pid }) + "\n");
}
