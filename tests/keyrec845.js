// Issue #845 recorder: the reporter's keyrec.js, plus an optional second
// argument "w32" that also asks the console for win32 input mode
// (CSI ?9001h) the way Ink based TUIs such as Claude Code do on Windows.
// One line per stdin 'data' event: JSON string and hex.  Ctrl+C exits.
const fs = require('fs');
const out = process.argv[2];
const w32 = process.argv[3] === 'w32';
process.stdin.setRawMode(true);
process.stdin.resume();
if (w32) process.stdout.write('\x1b[?9001h');
fs.writeFileSync(out, 'start isTTY=' + process.stdin.isTTY + ' w32=' + w32 + '\n');
process.stdout.write('keyrec845 ready\r\n');
process.stdin.on('data', d => {
  fs.appendFileSync(out, JSON.stringify(d.toString()) + ' hex=' + d.toString('hex') + '\n');
  if (d[0] == 3) process.exit();
});
