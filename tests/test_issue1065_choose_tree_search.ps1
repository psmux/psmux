# Issue #1065: search in the choosers, as tmux's tree mode has it.
#
# tmux `mode_tree_key` (mode-tree.c:1769-1784) opens a `(search) ` prompt on
# `/`, `?` or `C-s`; Enter moves the cursor to the next item the text hits,
# and `n` / `N` repeat the search forward and backward. The tree
# (window_tree_search) matches a session by its name, a window by its name
# and a pane by the command running in it; the buffer list
# (window_buffer_search) matches a buffer's name or its content. A search
# written all in lower case ignores case.
#
# psmux had none of it: `/` did nothing and the typed text fell on the
# chooser's own keys.
#
# Every check here drives a real attached client with keys written into its
# console input (tests/injector.cs) and reads the result back over the CLI:
# which session and window the client is on after Enter, or which buffer
# `d` removed. The suite runs under its own -L namespace and its own
# PSMUX_DATA_DIR, so the choosers see only the sessions it made.

$ErrorActionPreference = "Continue"
$script:pass = 0
$script:fail = 0
$script:results = @()

function Write-Test($msg) { Write-Host "  TEST: $msg" -ForegroundColor Yellow }
function Add-Result($name, $ok, $detail) {
    if ($ok) { Write-Host "  PASS: $name $detail" -ForegroundColor Green; $script:pass++ }
    else { Write-Host "  FAIL: $name $detail" -ForegroundColor Red; $script:fail++ }
    $script:results += [PSCustomObject]@{ Test = $name; Pass = $ok; Detail = $detail }
}

. "$PSScriptRoot\isolated_config.ps1"

$env:PSMUX_SESSION = ""
$env:PSMUX_RUN_NOPAUSE = "1"
$NS = "t1065_" + [guid]::NewGuid().ToString('N').Substring(0, 8)
$script:dataDir = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_1065_" + [guid]::NewGuid().ToString('N').Substring(0, 10))
New-Item -ItemType Directory -Force $script:dataDir | Out-Null
$env:PSMUX_DATA_DIR = $script:dataDir

$PSMUX = $env:PSMUX_TEST_BIN
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { $cmd = Get-Command psmux -EA SilentlyContinue; if ($cmd) { $PSMUX = $cmd.Source } }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

Write-Host "`n=== #1065 chooser search ===" -ForegroundColor Cyan
Write-Host "  Binary:    $PSMUX"
Write-Host "  Namespace: $NS"
Write-Host "  Data dir:  $script:dataDir"

function P { & $PSMUX -L $NS @args 2>$null }

$injectorExe = "$env:TEMP\psmux_injector.exe"
$injectorSrc = Join-Path $PSScriptRoot "injector.cs"
if (-not (Test-Path $injectorExe) -or ((Get-Item $injectorSrc).LastWriteTime -gt (Get-Item $injectorExe).LastWriteTime)) {
    $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
    & $csc /nologo /optimize /out:$injectorExe $injectorSrc 2>&1 | Out-Null
}
if (-not (Test-Path $injectorExe)) { Add-Result "injector compiled" $false $injectorExe; exit 1 }

# ---- the sessions the choosers will list --------------------------------
# alpha: 0 first
# beta:  0 editor, 1 logs
# gamma: 0 other (running ping), 1 logs2
P new-session -d -s alpha -n first | Out-Null
P new-session -d -s beta -n editor | Out-Null
P new-window -d -t beta -n logs | Out-Null
P new-session -d -s gamma -n other | Out-Null
P new-window -d -t gamma -n logs2 | Out-Null
Start-Sleep -Milliseconds 1500
P send-keys -t gamma:0 "ping -n 600 127.0.0.1" Enter | Out-Null
Start-Sleep -Milliseconds 2500

Write-Test "CLI: the server reports the command running in each pane"
$cmds = (P list-panes -s -t gamma -F "#{pane_id} #{pane_current_command}") -join " | "
$pingSeen = $cmds -match "(?i)ping"
Add-Result "gamma:0 runs ping" $pingSeen "[$cmds]"

function Where-Client {
    return ((P list-clients -F '#{client_session}:#{window_index}:#{window_name}') -join ",").Trim()
}

# One attached client per check, on alpha, so every check starts the same.
function Run-Keys($open, $keys) {
    P switch-client -t alpha | Out-Null
    $p = Start-Process -FilePath $PSMUX -ArgumentList "-L", $NS, "attach", "-t", "alpha" -PassThru
    Start-Sleep -Seconds 3
    & $injectorExe $p.Id "^b{SLEEP:500}$open{SLEEP:1500}$keys{SLEEP:1500}" | Out-Null
    Start-Sleep -Milliseconds 800
    $where = Where-Client
    try { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } catch {}
    Start-Sleep -Milliseconds 800
    return $where
}

# ---- the tree, prefix w ---------------------------------------------------
Write-Test "prefix w, /logs Enter, Enter: the client goes to beta:1 logs"
$w = Run-Keys "w" "/logs{ENTER}{SLEEP:700}{ENTER}"
Add-Result "tree: /logs finds the window" ($w -eq "beta:1:logs") "client=$w"

Write-Test "prefix w, /logs Enter, n, Enter: n repeats to gamma:1 logs2"
$w = Run-Keys "w" "/logs{ENTER}{SLEEP:700}n{SLEEP:700}{ENTER}"
Add-Result "tree: n goes to the next hit" ($w -eq "gamma:1:logs2") "client=$w"

Write-Test "prefix w, /logs Enter, n, N, Enter: N goes back to beta:1 logs"
$w = Run-Keys "w" "/logs{ENTER}{SLEEP:700}n{SLEEP:700}N{SLEEP:700}{ENTER}"
Add-Result "tree: N goes to the previous hit" ($w -eq "beta:1:logs") "client=$w"

Write-Test "prefix w, ?gamma Enter, Enter: ? opens the same prompt"
$w = Run-Keys "w" "?gamma{ENTER}{SLEEP:700}{ENTER}"
Add-Result "tree: ? searches, a session hit switches to it" ($w -like "gamma:*") "client=$w"

Write-Test "prefix w, C-s logs Enter, Enter: C-s opens the same prompt"
$w = Run-Keys "w" "^slogs{ENTER}{SLEEP:700}{ENTER}"
Add-Result "tree: C-s searches" ($w -eq "beta:1:logs") "client=$w"

Write-Test "prefix w, /LOGS Enter, Enter: upper case is exact, so nothing moves"
$w = Run-Keys "w" "/LOGS{ENTER}{SLEEP:700}{ENTER}"
Add-Result "tree: /LOGS finds nothing, Enter takes the row it opened on" ($w -eq "alpha:0:first") "client=$w"

Write-Test "prefix w, /zzz Escape, /logs Enter, Enter: Escape closes only the prompt"
$w = Run-Keys "w" "/zzz{SLEEP:300}{ESC}{SLEEP:700}/logs{ENTER}{SLEEP:700}{ENTER}"
Add-Result "tree: the chooser survives Escape at the prompt" ($w -eq "beta:1:logs") "client=$w"

Write-Test "prefix w, /ping Enter, Enter: a pane is found by its command"
$w = Run-Keys "w" "/ping{ENTER}{SLEEP:1200}{ENTER}"
Add-Result "tree: /ping finds the pane in gamma:0" ($w -eq "gamma:0:other") "client=$w"

# ---- the session list, prefix s -------------------------------------------
Write-Test "prefix s, /gam Enter, Enter: the client goes to gamma"
$w = Run-Keys "s" "/gam{ENTER}{SLEEP:700}{ENTER}"
Add-Result "sessions: /gam finds gamma" ($w -like "gamma:*") "client=$w"

Write-Test "prefix s, /a Enter, n, Enter: n walks to the next session hit"
# rows: alpha, beta, gamma; cursor on alpha; /a hits beta, then n gamma
$w = Run-Keys "s" "/a{ENTER}{SLEEP:700}n{SLEEP:700}{ENTER}"
Add-Result "sessions: n repeats" ($w -like "gamma:*") "client=$w"

# ---- the buffer list, prefix = --------------------------------------------
# The buffer whose content holds the needle is set second and sorts second, so it is the
# middle row whichever end the list starts from, never the row the chooser
# opens on: a search that did nothing would delete another buffer.
P set-buffer -t alpha -b ba "plain" | Out-Null
P set-buffer -t alpha -b bb "has needle inside" | Out-Null
P set-buffer -t alpha -b bc "plain too" | Out-Null
Start-Sleep -Milliseconds 400
$bufs0 = ((P list-buffers -t alpha) | ForEach-Object { ($_ -split ':')[0] }) -join ","
Add-Result "buffers: three to start with" (($bufs0 -split ",").Count -eq 3) "buffers=$bufs0"
Write-Test "prefix =, /needle Enter, d: the buffer whose CONTENT hits is the one deleted"
$null = Run-Keys "=" "/needle{ENTER}{SLEEP:900}d"
$bufs = ((P list-buffers -t alpha) | ForEach-Object { ($_ -split ':')[0] }) -join ","
Add-Result "buffers: bb deleted, the others kept" (($bufs -notmatch "bb") -and ($bufs -match "ba") -and ($bufs -match "bc")) "buffers=$bufs"

# ---- cleanup -------------------------------------------------------------
P kill-server | Out-Null
Start-Sleep -Milliseconds 500
Remove-PsmuxIsolatedConfig
$env:PSMUX_DATA_DIR = $null
Remove-Item -Recurse -Force $script:dataDir -EA SilentlyContinue

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $pass / $($pass + $fail)" -ForegroundColor $(if ($fail -eq 0) { 'Green' } else { 'Yellow' })
foreach ($r in $results) {
    $status = if ($r.Pass) { 'PASS' } else { 'FAIL' }
    Write-Host "  [$status] $($r.Test) $($r.Detail)" -ForegroundColor $(if ($r.Pass) { 'Green' } else { 'Red' })
}
exit $fail
