# Make a copy of test_pane_startup_perf.ps1 that runs in its own -L namespace.
$src = 'C:\Users\godwin\Documents\workspace\psmux-cgate\tests\test_pane_startup_perf.ps1'
$dst = 'C:\Users\godwin\Documents\workspace\psmux-cgate\tests\spf_ns.ps1'
$t = [IO.File]::ReadAllText($src)
$t = $t.Replace('& $PSMUX ', '& $PSMUX -L $KpNs ')
$t = $t.Replace('-FilePath $PSMUX -ArgumentList "', '-FilePath $PSMUX -ArgumentList "-L", $KpNs, "')
$t = $t.Replace('"$PSMUX_DIR\${SessionName}.port"', '"$PSMUX_DIR\$($KpNs)__${SessionName}.port"')
$t = $t.Replace('"$PSMUX_DIR\${SessionName}.key"', '"$PSMUX_DIR\$($KpNs)__${SessionName}.key"')
$t = $t.Replace('"$PSMUX_DIR\perf_test_*.port"', '"$PSMUX_DIR\$($KpNs)__perf_test_*.port"')
$t = $t.Replace('"$PSMUX_DIR\perf_test_*.key"', '"$PSMUX_DIR\$($KpNs)__perf_test_*.key"')
$t = $t.Replace('"$PSMUX_DIR\__warm__.port"', '"$PSMUX_DIR\$($KpNs)____warm__.port"')
# define the namespace right after the PSMUX_DIR line
$anchor = '$PSMUX_DIR = if ($env:PSMUX_DATA_DIR)'
$i = $t.IndexOf($anchor)
$t = $t.Insert($i, '$KpNs = "spf$PID"' + "`r`n")
[IO.File]::WriteAllText($dst, $t)
# report every psmux invocation that still lacks -L
$n = 0
foreach ($l in [IO.File]::ReadAllLines($dst)) {
  $n++
  if ($l -match '\$PSMUX\b' -and $l -notmatch '-L \$KpNs|"-L", \$KpNs|\$PSMUX_DIR|\$PSMUX = |Resolve-Path \$PSMUX|Test-Path \$PSMUX|Write-Host|Binary \$PSMUX|\$PSMUX\)|-not \$PSMUX') { "$n : $($l.Trim())" }
}
