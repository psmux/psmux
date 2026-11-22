// Issue #556 timing probe: emulates yazi's startup probe ordering.
//
// yazi sends a probe bundle (DA1 + OSC 11 bg query + others) and reads
// responses in a window that CLOSES when the DA1 answer arrives (or 3s).
// Any response landing after the DA1 answer is outside the window and gets
// re-parsed by yazi's event layer as interactive input (the "Shell:" popup).
//
// This probe sends OSC 11 + CSI ?996n FIRST, then DA1 (same order yazi uses:
// color queries are emitted before the closing DA1), records the arrival
// time of every stdin chunk, and reports:
//   da1Ms    - ms until the DA1 answer (CSI ... c) arrived
//   osc11Ms  - ms until the OSC 11 answer arrived (null if never)
//   raceLost - true when OSC 11 arrived AFTER DA1 (outside yazi's window)
//
// Env: PROBE_OUT - result JSON path (default probe_timing.json)
//      PROBE_PAINT=1 - write a "." right after the bundle, in the same write.
//                      The Windows 10 inbox conhost (build 19045) holds a
//                      query written on its own and hands it to psmux only
//                      with the next text it draws, so without text after it
//                      no answer comes there.
const fs = require("fs");
const t0 = process.hrtime.bigint();
const events = [];
let buf = Buffer.alloc(0);
let da1Ms = null;
let osc11Ms = null;
let schemeMs = null;

if (process.stdin.isTTY) process.stdin.setRawMode(true);
process.stdin.resume();
process.stdin.on("data", (chunk) => {
  const tMs = Number(process.hrtime.bigint() - t0) / 1e6;
  events.push({ tMs: Math.round(tMs * 10) / 10, hex: chunk.toString("hex") });
  buf = Buffer.concat([buf, chunk]);
  const text = buf.toString("latin1");
  // DA1 answer: CSI [?] Ps ; ... c
  if (da1Ms === null && /\x1b\[\?[\d;]*c/.test(text)) da1Ms = tMs;
  // OSC 11 answer: ESC ] 11 ; rgb:... (BEL | ESC \)
  if (osc11Ms === null && /\x1b\]11;rgb:[^\x07\x1b]+(?:\x07|\x1b\\)/.test(text)) osc11Ms = tMs;
  // scheme answer: CSI ?997;{1|2}n
  if (schemeMs === null && /\x1b\[\?997;[12]n/.test(text)) schemeMs = tMs;
});

// yazi-order bundle: color queries first, DA1 last (DA1 closes the window)
// PROBE_TERM=bel (default, yazi's form) or st (ESC \ terminator, #473 form)
// PROBE_BUNDLE selects composition/order:
//   yazi     - OSC11, ?996n, DA1   (default; yazi's probe shape)
//   noda1    - OSC11, ?996n        (yazi order without the DA1)
//   osc11    - OSC11 alone
//   rev      - ?996n, OSC11        (#473 order, no DA1)
//   revda1   - ?996n, OSC11, DA1   (#473 order plus DA1)
const osc11q = process.env.PROBE_TERM === "st" ? "\x1b]11;?\x1b\\" : "\x1b]11;?\x07";
const osc10q = process.env.PROBE_TERM === "st" ? "\x1b]10;?\x1b\\" : "\x1b]10;?\x07";
const osc4s = Array.from({ length: 16 }, (_, i) => `\x1b]4;${i};?\x1b\\`).join("");
const bundles = {
  yazi: osc11q + "\x1b[?996n" + "\x1b[c",
  noda1: osc11q + "\x1b[?996n",
  osc11: osc11q,
  rev: "\x1b[?996n" + osc11q,
  revda1: "\x1b[?996n" + osc11q + "\x1b[c",
  b473: "\x1b[?996n" + osc10q + osc11q + osc4s, // exact #473 "all" burst
  b473small: "\x1b[?996n" + osc10q + osc11q,    // #473 minus the OSC 4s
  osc1011: osc10q + osc11q,                       // OSC 10 then 11 only
  osc114: osc11q + osc4s,                         // OSC 11 then the OSC 4s
};
process.stdout.write(bundles[process.env.PROBE_BUNDLE || "yazi"] + (process.env.PROBE_PAINT === "1" ? "." : ""));

setTimeout(() => {
  const result = {
    da1Ms: da1Ms === null ? null : Math.round(da1Ms * 10) / 10,
    osc11Ms: osc11Ms === null ? null : Math.round(osc11Ms * 10) / 10,
    schemeMs: schemeMs === null ? null : Math.round(schemeMs * 10) / 10,
    raceLost:
      da1Ms !== null && (osc11Ms === null || osc11Ms > da1Ms),
    events,
    responseHex: buf.toString("hex"),
  };
  fs.writeFileSync(process.env.PROBE_OUT || "probe_timing.json", JSON.stringify(result));
  console.log("TIMING_PROBE_DONE");
  process.exit(0);
}, 4000);
