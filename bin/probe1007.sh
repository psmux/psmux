#!/bin/bash
# probe1007.sh <out> <printf-format-queries>
out="$1"; q="$2"
old=$(stty -g)
stty raw -echo min 0 time 10
printf "$q"
reply=$(cat | sed -e 's/\x1b/\\e/g' -e 's/\x07/\\a/g')
stty "$old"
printf '%s\n' "$reply" > "$out"
