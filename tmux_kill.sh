#!/bin/bash
# tmux baseline: time kill-pane and a display-message right after it.
ns=kp$$
tmux -f /dev/null -L $ns new-session -d -s s -x 160 -y 50
for i in $(seq 1 30); do
  tmux -L $ns split-window -v 'sleep 300'
  sleep 0.2
  t0=$(date +%s%N)
  tmux -L $ns kill-pane
  t1=$(date +%s%N)
  tmux -L $ns display-message -p alive >/dev/null
  t2=$(date +%s%N)
  echo "$(( (t1-t0)/1000 )) $(( (t2-t1)/1000 ))"
done | awk '{k[NR]=$1; d[NR]=$2} END {n=asort(k); asort(d); printf "tmux kill-pane us p50=%d p90=%d max=%d | display-message after us p50=%d p90=%d max=%d\n", k[int(n/2)], k[int(n*0.9)], k[n], d[int(n/2)], d[int(n*0.9)], d[n]}'
tmux -L $ns kill-server
