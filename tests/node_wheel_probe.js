// Node model of a Claude Code style full screen TUI, for issue #613.
//
// It is a node app, so `process.stdin.setRawMode(true)` goes through libuv's
// uv_tty_set_mode, which writes ENABLE_WINDOW_INPUT | ENABLE_VIRTUAL_TERMINAL_INPUT
// (0x0208) over the WHOLE console input mode word.  ENABLE_MOUSE_INPUT is gone from
// the moment raw mode is entered, whether or not the app asked for the mouse first.
// That is the shape of every long lived Claude Code pane measured in #613.
//
// Usage: node node_wheel_probe.js <logPath> <decset 0|1> [rawFirst 0|1]
//   decset=1   emit the mouse DECSET the way Claude Code 2.1.250 does
//   decset=0   emit no mouse DECSET at all, the shape the reporter measured
//   rawFirst=1 enter raw mode BEFORE the DECSET instead of after
//
// Every stdin chunk is appended to the log as hex, so the log is the ground truth for
// "what exactly did psmux forward into this pane for one wheel notch".
'use strict';
const fs = require('fs');
const path = require('path');

const log = process.argv[2] || path.join(process.env.TEMP || '.', 'psmux_node_wheel.log');
const decset = process.argv[3] !== '0';
const rawFirst = process.argv[4] === '1';

fs.writeFileSync(log, `NODE_WHEEL START decset=${decset} rawFirst=${rawFirst} pid=${process.pid}\n`);

function raw() {
  try {
    process.stdin.setRawMode(true);
    fs.appendFileSync(log, 'RAWMODE_SET\n');
  } catch (e) {
    fs.appendFileSync(log, 'RAWMODE_FAILED ' + e.message + '\n');
  }
}

if (rawFirst) raw();

process.stdout.write('\x1b[?1049h\x1b[H\x1b[2J');
if (decset) process.stdout.write('\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h');
process.stdout.write('\x1b[HNODE_WHEEL_READY\r\n');

if (!rawFirst) raw();
process.stdin.resume();

process.stdin.on('data', (buf) => {
  let hex = '';
  let txt = '';
  for (const b of buf) {
    hex += b.toString(16).toUpperCase().padStart(2, '0') + ' ';
    if (b === 0x1b) txt += '<ESC>';
    else if (b >= 0x20 && b < 0x7f) txt += String.fromCharCode(b);
    else txt += '<' + b.toString(16).toUpperCase().padStart(2, '0') + '>';
  }
  fs.appendFileSync(log, `RECV ${txt}  |  ${hex.trim()}\n`);
  for (const b of buf) {
    if (b === 0x1a) { // Ctrl+Z quits
      if (decset) process.stdout.write('\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l');
      process.stdout.write('\x1b[?1049l');
      fs.appendFileSync(log, 'NODE_WHEEL END\n');
      process.exit(0);
    }
  }
});
