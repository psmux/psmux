#!/bin/bash
D=/mnt/c/Users/godwin/Documents/workspace/psmux-pr1008/bin
NS=t1007_$$
T="${TMUXBIN:-tmux} -f /dev/null -L $NS"
$T new-session -d -s s -x 120 -y 20
sleep 0.5
i=0
run() { # name, queries, [setargs...]
  i=$((i+1)); name="$1"; q="$2"; shift 2
  $T new-window -d -t s -n w$i; sleep 0.3
  if [ $# -gt 0 ]; then if [ "$1" = -p ]; then $T set -p -t s:w$i "$2" "$3"; else $T set "$@"; fi; fi
  rm -f /tmp/r1007_$i
  $T send-keys -t s:w$i "bash $D/probe1007.sh /tmp/r1007_$i '$q'" Enter
  for k in $(seq 50); do [ -f /tmp/r1007_$i ] && break; sleep 0.1; done
  echo "$name => $(cat /tmp/r1007_$i 2>/dev/null || echo NOFILE)"
  if [ "$1" = -g ]; then $T set -gu "$2"; fi
}
E='\033'; A='\007'; ST='\033\\'
run "index 1 BEL" "$E]4;1;?$A"
run "index 1 ST" "$E]4;1;?$ST"
run "index 20 none BEL" "$E]4;20;?$A"
run "index 20 set by OSC4" "$E]4;20;rgb:11/22/33$ST$E]4;20;?$A"
run "index 255 set by OSC4" "$E]4;255;#abcdef$A$E]4;255;?$ST"
run "low and high" "$E]4;20;#112233;1;?;20;?;21;?;2;?$ST"
run "reset by 104" "$E]4;20;#112233$A$E]104;20$A$E]4;20;?$A"
run "fg BEL" "$E]10;?$A"
run "bg ST" "$E]11;?$ST"
run "pane-colours -p 20" "$E]4;20;?$A" -p 'pane-colours[20]' '#445566'
run "pane-colours -g 200" "$E]4;200;?$ST" -g 'pane-colours[200]' '#123456'
$T kill-server
