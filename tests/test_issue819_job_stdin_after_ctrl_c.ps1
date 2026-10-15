# Issue #819: server side job children must not inherit the server's stdio.
#
#   psmux if-shell 'exit 0 # #{pid}' 'display -p TRUE' 'display -p FALSE'   # TRUE
#   psmux send-keys -t s:0.0 C-c
#   psmux if-shell 'exit 0 # #{pid}' 'display -p TRUE' 'display -p FALSE'   # FALSE (bug)
#
# The server is born with a hidden console of its own, so its std handle
# slots hold that console's handles. Relaying C-c to a pane briefly
# FreeConsole()s and AttachConsole()s to the pane's console, and a server
# never re-attaches afterwards, so the slots are left holding handles that
# no longer exist. A child spawned with inherited stdio then fails in
# CreateProcess setup (DuplicateHandle, os error 6) before it runs.
#
# tmux never lets a job inherit the server's stdio: job_run (job.c:156)
# points the child's stdin and stdout at the job socket and stderr at the
# socket or /dev/null. Every job psmux starts in the server is checked here
# after C-c: forwarded if-shell (with and without -b), run-shell and
# run-shell -b, #() format jobs, if-shell and run-shell inside source-file,
# and if-shell and run-shell -b from a hook.
#
# Runs in its own -L namespace with a private PSMUX_DATA_DIR.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path $env:TEMP ("psmux_issue819_data_" + $PID)
    New-Item -ItemType Directory -Force -Path $env:PSMUX_DATA_DIR | Out-Null
}
$dataDir = $env:PSMUX_DATA_DIR
$TMP = Join-Path $env:TEMP ("psmux_issue819_" + $PID)
New-Item -ItemType Directory -Force -Path $TMP | Out-Null
$SOCK = "i819_$PID"
$script:Pass = 0; $script:Fail = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }
function Write-Ascii([string]$p, [string]$t) { [IO.File]::WriteAllText($p, $t, (New-Object System.Text.ASCIIEncoding)) }
function P { & $PSMUX -L $SOCK @args 2>&1 | Out-String | ForEach-Object { $_.Trim() } }
function Check([string]$what, [string]$got, [string]$want) {
    if ($got -ceq $want) { Write-Pass "$what = '$want'" } else { Write-Fail "$what = '$got' (expected '$want')" }
}
function Wait-Port([string]$sess) {
    for ($i = 0; $i -lt 80; $i++) { if (Test-Path "$dataDir\${SOCK}__$sess.port") { return }; Start-Sleep -Milliseconds 250 }
}
function Wait-File([string]$path) {
    for ($i = 0; $i -lt 60; $i++) { if (Test-Path $path) { return $true }; Start-Sleep -Milliseconds 250 }
    return $false
}
function Check-File([string]$what, [string]$path) {
    if (Wait-File $path) { Write-Pass "$what ran (wrote $(Split-Path $path -Leaf))" } else { Write-Fail "$what never ran (no $(Split-Path $path -Leaf))" }
}
function Q([string]$s) { "'" + $s.Replace("'", "''") + "'" }

# Every job check, run once before and once after C-c. $tag keeps the
# marker files and options of the two rounds apart.
function Run-Jobs([string]$tag) {
    Check "[$tag] forwarded if-shell 'exit 0 # #{pid}'" (P if-shell 'exit 0 # #{pid}' 'display-message -p TRUE' 'display-message -p FALSE') "TRUE"
    Check "[$tag] forwarded if-shell 'exit 1 # #{pid}'" (P if-shell 'exit 1 # #{pid}' 'display-message -p TRUE' 'display-message -p FALSE') "FALSE"

    P set -gu "@bg_$tag" | Out-Null
    P if-shell -b 'exit 0 # #{pid}' "set -g @bg_$tag yes" "set -g @bg_$tag no" | Out-Null
    $v = ""
    for ($i = 0; $i -lt 40; $i++) { $v = P show -gv "@bg_$tag"; if ($v -ceq "yes" -or $v -ceq "no") { break }; Start-Sleep -Milliseconds 250 }
    Check "[$tag] forwarded if-shell -b" $v "yes"

    $rs = P run-shell 'Write-Output "RS#{pid}"'
    Check "[$tag] run-shell output" $rs "RS$serverPid"

    $f = Join-Path $TMP "runb_$tag.txt"
    $out = P run-shell -b ("Set-Content -LiteralPath " + (Q $f) + " -Value ok")
    if ($out) { Write-Info "run-shell -b said: $out" }
    Check-File "[$tag] run-shell -b" $f

    Check "[$tag] #() format job" (P display-message -p '#(echo JOB)') "JOB"

    $f = Join-Path $TMP "cfgrun_$tag.txt"
    $conf = Join-Path $TMP "jobs_$tag.conf"
    Write-Ascii $conf ("if-shell 'exit 0' 'set -g @cfg_$tag yes' 'set -g @cfg_$tag no'`n" +
        "run-shell -b " + '"' + "Set-Content -LiteralPath '$f' -Value ok" + '"' + "`n")
    P source-file $conf | Out-Null
    Check "[$tag] source-file if-shell" (P show -gv "@cfg_$tag") "yes"
    Check-File "[$tag] source-file run-shell -b" $f

    $f = Join-Path $TMP "hookrun_$tag.txt"
    P set-hook -g after-rename-window ("if-shell 'exit 0' 'set -g @hook_$tag yes' 'set -g @hook_$tag no' ; run-shell -b " + '"' + "Set-Content -LiteralPath '$f' -Value ok" + '"') | Out-Null
    P rename-window -t s:0 "w_$tag" | Out-Null
    $v = ""
    for ($i = 0; $i -lt 40; $i++) { $v = P show -gv "@hook_$tag"; if ($v -ceq "yes" -or $v -ceq "no") { break }; Start-Sleep -Milliseconds 250 }
    Check "[$tag] hook if-shell" $v "yes"
    Check-File "[$tag] hook run-shell -b" $f
    P set-hook -gu after-rename-window | Out-Null
}

Write-Host "binary:  $PSMUX" -ForegroundColor Cyan
Write-Host "socket:  $SOCK" -ForegroundColor Cyan
$plain = Join-Path $TMP "plain.conf"
Write-Ascii $plain "set -g @plain 1`n"

try {
    & $PSMUX -L $SOCK -f $plain new-session -d -s s -x 100 -y 30 -- cmd.exe /d /k 2>&1 | Out-Null
    Wait-Port s; Start-Sleep -Milliseconds 800
    $serverPid = P display-message -p '#{pid}'
    Write-Info "server pid $serverPid"

    Write-Host "`n=== Part A: every job kind before C-c ===" -ForegroundColor Cyan
    Run-Jobs "pre"

    Write-Host "`n=== Part B: the issue's sequence, then every job kind again ===" -ForegroundColor Cyan
    P send-keys -t s:0.0 -l 'echo benign' | Out-Null
    P send-keys -t s:0.0 Enter | Out-Null
    $ready = $false
    for ($i = 0; $i -lt 40; $i++) {
        if ((P capture-pane -p -t s:0.0) -split "`n" | Where-Object { $_.Trim() -eq 'benign' }) { $ready = $true; break }
        Start-Sleep -Milliseconds 125
    }
    if ($ready) { Write-Pass "pane ran 'echo benign'" } else { Write-Fail "pane never printed 'benign'" }
    P send-keys -t s:0.0 C-c | Out-Null
    Start-Sleep -Milliseconds 500
    Check "server is the same process after C-c" (P display-message -p '#{pid}') $serverPid
    Run-Jobs "post"
} finally {
    & $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
    Remove-Item -Recurse -Force $TMP -ErrorAction SilentlyContinue
}

Write-Host "`nResults: $script:Pass passed, $script:Fail failed" -ForegroundColor $(if ($script:Fail) { "Red" } else { "Green" })
exit $script:Fail
