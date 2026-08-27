// Byte logger for issue #610.  Runs inside a psmux pane, puts stdin in raw mode
// (which is what a node TUI such as Claude Code does) and records the exact
// bytes psmux writes into the pane, plus what node's own readline key decoder
// makes of them.
const fs = require('fs');
const readline = require('readline');
const log = process.argv[2];
fs.writeFileSync(log, 'NODE READY pid=' + process.pid + '\n');
function w(s) { try { fs.appendFileSync(log, s + '\n'); } catch (e) { /* ignore */ } }
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
