// delayed-ui.ts — TEST ONLY. An ordinary extension that is slow in its
// `ui_prompt_start` handler. Loaded BEFORE an observer of the UI prompt
// events it makes that observer hear `ui_prompt_end` before
// `ui_prompt_start` for a prompt that has already closed: the reason Deck
// reports no input request for Pi. Never packaged.
import fs from "node:fs";

export default function (pi) {
  const note = (event) => {
    if (process.env.FIXTURE_LOG) fs.appendFileSync(process.env.FIXTURE_LOG, `${JSON.stringify({ event, pid: process.pid, t: Date.now() })}\n`);
  };
  pi.on("ui_prompt_start", async () => {
    note("delayed_start_received");
    await new Promise((resolve) => setTimeout(resolve, 3000));
    note("delayed_start_released");
  });
  pi.on("ui_prompt_end", () => note("delayed_end_received"));
}
