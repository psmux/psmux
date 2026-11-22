// Issue #473 test probe: emulates an application (GitHub Copilot CLI style)
// that queries the terminal for its colors and reads the replies on stdin.
//
// Env:
//   PROBE_OUT     - file to write the JSON result to (default probe_result.json)
//   PROBE_QUERIES - "all" (default: ?996n + OSC10 + OSC11 + OSC4 0..15),
//                   "idx5" (single OSC 4;5;? query),
//                   "scheme" (only CSI ?996n)
//   PROBE_PAINT   - "1" writes a `.` after the queries, in the same write. The
//                   Windows 10 inbox conhost (build 19045) hands psmux the
//                   queries only when it next draws something, so without it
//                   none reaches psmux; the suite sets it on a build before 22000.
const fs = require("fs");
const chunks = [];

if (process.stdin.isTTY) {
  process.stdin.setRawMode(true);
}
process.stdin.resume();
process.stdin.on("data", (chunk) => chunks.push(chunk));

const mode = process.env.PROBE_QUERIES || "all";
let queries;
if (mode === "idx5") {
  queries = ["\x1b]4;5;?\x1b\\"];
} else if (mode === "scheme") {
  queries = ["\x1b[?996n"];
} else {
  queries = [
    "\x1b[?996n",
    "\x1b]10;?\x1b\\",
    "\x1b]11;?\x1b\\",
    ...Array.from({ length: 16 }, (_, i) => `\x1b]4;${i};?\x1b\\`),
  ];
}
process.stdout.write(queries.join("") + (process.env.PROBE_PAINT === "1" ? "." : ""));

setTimeout(() => {
  const text = Buffer.concat(chunks).toString("utf8");
  const oscEnd = "(?:\\x07|\\x1b\\\\)";
  const palette = {};
  for (const m of text.matchAll(
    new RegExp(`\\x1b\\]4;(\\d+);(rgb:[^\\x07\\x1b]+)${oscEnd}`, "g"),
  )) {
    palette[m[1]] = m[2];
  }
  const schemeMatch = text.match(/\x1b\[\?997;([12])n/);
  const fgMatch = text.match(new RegExp(`\\x1b\\]10;(rgb:[^\\x07\\x1b]+)${oscEnd}`));
  const bgMatch = text.match(new RegExp(`\\x1b\\]11;(rgb:[^\\x07\\x1b]+)${oscEnd}`));

  const result = JSON.stringify({
    scheme: schemeMatch ? Number(schemeMatch[1]) : null,
    fg: fgMatch ? fgMatch[1] : null,
    bg: bgMatch ? bgMatch[1] : null,
    palette,
    paletteCount: Object.keys(palette).length,
    responseHex: Buffer.concat(chunks).toString("hex"),
  });
  fs.writeFileSync(process.env.PROBE_OUT || "probe_result.json", result);
  console.log("PROBE_DONE");
  process.exit(0);
}, 2500);
