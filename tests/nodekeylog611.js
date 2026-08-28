// Byte logger for issue #611 (Shift+Enter / ESC+CR on the LOCAL client path).
//
// Same idea as nodekeylog610.js but every line carries a millisecond timestamp,
// because half of #611 is a TIMING claim: an ESC immediately followed by a CR
// has to reach the child as one atomic pair, not as two writes separated by a
// gap.  A readline style app (node's own decoder, ink, Claude Code) turns
// "1b 0d in one read" into name=return meta=true, and "1b" then "0d" a while
// later into "cancel" followed by "submit".  Only the timestamps can tell the
// two apart from the outside.
const fs = require('fs');
const readline = require('readline');
const log = process.argv[2];
const t0 = Date.now();
fs.writeFileSync(log, 'NODE READY pid=' + process.pid + '\n');
function w(s) { try { fs.appendFileSync(log, (Date.now() - t0) + ' ' + s + '\n'); } catch (e) { /* ignore */ } }
process.stdin.setRawMode(true);
process.stdin.resume();
process.stdin.on('data', (b) => {
  w('BYTES hex=[ ' + [...b].map(x => x.toString(16).padStart(2, '0')).join(' ') + ' ]');
  if (b.length === 1 && b[0] === 0x1a) { w('DONE'); process.exit(0); }
});
readline.emitKeypressEvents(process.stdin);
process.stdin.on('keypress', (str, key) => {
  if (!key) return;
  w('KEY name=' + key.name + ' ctrl=' + key.ctrl + ' meta=' + key.meta + ' shift=' + key.shift);
});
setTimeout(() => { w('TIMEOUT'); process.exit(0); }, 180000);
