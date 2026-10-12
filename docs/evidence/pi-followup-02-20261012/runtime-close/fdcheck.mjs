// How many descriptors this process holds around one connection to a unix
// socket: before it, while it is open, right after destroy(), and at 'close'.
import net from "node:net";
import fs from "node:fs";
const path = process.argv[2];
const fds = () => fs.readdirSync("/dev/fd").length;
const tick = () => new Promise((r) => setImmediate(r));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const out = { runtime: process.versions.bun ? `bun ${process.versions.bun}` : `node ${process.versions.node}`, cases: [] };

async function one(name, when) {
  const before = fds();
  const s = net.createConnection({ path });
  const sync = fds();
  let closedAt = null;
  const closed = new Promise((r) => s.on("close", () => { closedAt = fds(); r(); }));
  s.on("error", () => {});
  let afterDestroy = null, afterTick = null, open = null;
  const destroy = () => { open = fds(); s.destroy(); afterDestroy = fds(); };
  if (when === "at-once") destroy();
  else if (when === "connected") await new Promise((r) => s.on("connect", () => { destroy(); r(); }));
  else if (when === "written") await new Promise((r) => s.on("connect", () => { s.write("{}\n", () => { destroy(); r(); }); }));
  else if (when === "answered") await new Promise((r) => { s.on("connect", () => s.write("{}\n")); s.on("data", () => { destroy(); r(); }); });
  else if (when === "peer-closed") await new Promise((r) => { s.on("connect", () => s.write("{}\n")); s.on("end", () => { destroy(); r(); }); s.resume(); });
  await tick(); afterTick = fds();
  await Promise.race([closed, sleep(1000)]);
  await sleep(50);
  out.cases.push({ name, before, afterCreate: sync, open, afterDestroy, afterTick, atClose: closedAt, later: fds() });
}
for (const when of ["at-once", "connected", "written", "answered", "peer-closed"]) {
  for (let i = 0; i < 3; i++) await one(when, when);
}
console.log(JSON.stringify(out));
