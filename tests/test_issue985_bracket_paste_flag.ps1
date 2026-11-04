# Issue #985: `#{bracket_paste_flag}` printed nothing.
#
# tmux 3.7 added the format (CHANGES: "Add bracket_paste_flag format flag",
# issue 4951): format.c format_cb_bracket_paste_flag answers "1" when the
# pane's screen has MODE_BRACKETPASTE (DECSET 2004) and "0" otherwise.
#
# MEASURED on 8a1e4fea (`-L r985`): a pane that wrote ESC[?2004h, the same
# pane after ESC[?2004l and a plain pane all printed '' for it.
#
# A client that embeds psmux asks this before it sends multi line text, to
# know whether the line breaks would be taken as Enter. So the checks are:
# per pane answers through display -t and list-panes -F, the conditional a
# client would write, DECRST, and an attached client window on top.
#
# Private PSMUX_DATA_DIR and a -L namespace; cleanup is `-L <ns> kill-server`
# and the attached window is closed by its own PID.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_BIN) { $env:PSMUX_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Check($name, $got, $want) {
    if ("$got" -ceq "$want") { Write-Pass "$name ($want)" } else { Write-Fail "$name`n      expected: [$want]`n      got:      [$got]" }
}

$defaultBefore = (& $PSMUX ls 2>&1 | Out-String).Trim()
# An attached client launched from a shell inside psmux would re-enter the
# caller's session; scrub the routing variables first.
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -and $_.Name -ne 'PSMUX_BIN' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
$savedShell = $env:SHELL
Remove-Item Env:SHELL -EA SilentlyContinue
$ROOT = Join-Path $env:TEMP "psmux_985_$PID"
New-Item -ItemType Directory -Force $ROOT | Out-Null
$env:PSMUX_DATA_DIR = $ROOT
$env:PSMUX_NO_WARM = "1"
$NS = "i985_$PID"
function P { (& $PSMUX -L $NS @args 2>&1 | ForEach-Object { "$_" }) -join "`n" }

# A pane child that writes `$seq`, then leaves a marker and waits.
$emit = Join-Path $ROOT "emit.ps1"
Set-Content $emit @'
param([string]$Seq, [string]$Mark)
$e = [char]27
[Console]::Write($Seq.Replace('\e', "$e"))
Set-Content $Mark 'done'
Start-Sleep 120
'@
function Emitter($seq, $mark) { "pwsh -NoProfile -NoLogo -File `"$emit`" -Seq `"$seq`" -Mark `"$mark`"" }
function Wait-Mark($mark) {
    for ($i = 0; $i -lt 80 -and -not (Test-Path $mark); $i++) { Start-Sleep -Milliseconds 250 }
    Start-Sleep -Milliseconds 400
    return (Test-Path $mark)
}

$client = $null
try {
    P new-session -d -s s -x 100 -y 24 "cmd.exe /k" | Out-Null
    for ($i = 0; $i -lt 40; $i++) { & $PSMUX -L $NS has-session -t s 2>$null; if ($LASTEXITCODE -eq 0) { break }; Start-Sleep -Milliseconds 250 }
    Start-Sleep -Seconds 1

    Write-Host "`n[1] A pane that never asked" -ForegroundColor Yellow
    Check "a cmd.exe pane reports 0, not an empty string" (P display -p -t s '#{bracket_paste_flag}') "0"

    Write-Host "`n[2] DECSET 2004 in one pane" -ForegroundColor Yellow
    $m1 = Join-Path $ROOT "m1"
    $p1 = P new-window -d -P -F '#{pane_id}' -t s -n on (Emitter '\e[?2004h' $m1)
    if (-not (Wait-Mark $m1)) { Write-Fail "the emitter pane never ran" }
    Check "display -t on that pane reports 1" (P display -p -t $p1 '#{bracket_paste_flag}') "1"
    Check "the other pane still reports 0" (P display -p -t 's:0' '#{bracket_paste_flag}') "0"
    Check "the conditional a client would ask" (P display -p -t $p1 '#{?bracket_paste_flag,safe,hold}') "safe"

    Write-Host "`n[3] list-panes -F answers per pane" -ForegroundColor Yellow
    $m2 = Join-Path $ROOT "m2"
    $p2 = P split-window -d -P -F '#{pane_id}' -t "s:on" (Emitter '\e[?2004l' $m2)
    if (-not (Wait-Mark $m2)) { Write-Fail "the second emitter pane never ran" }
    Check "two panes in one window" (P list-panes -t 's:on' -F '#{pane_id}=#{bracket_paste_flag}') "$p1=1`n$p2=0"

    Write-Host "`n[4] DECSET then DECRST in the same pane" -ForegroundColor Yellow
    $m3 = Join-Path $ROOT "m3"
    $p3 = P new-window -d -P -F '#{pane_id}' -t s -n off (Emitter '\e[?2004h\e[?2004l' $m3)
    if (-not (Wait-Mark $m3)) { Write-Fail "the third emitter pane never ran" }
    Check "a pane that turned it off again reports 0" (P display -p -t $p3 '#{bracket_paste_flag}') "0"

    Write-Host "`n[5] An attached client window" -ForegroundColor Yellow
    $client = Start-Process -FilePath $PSMUX -ArgumentList "-L", $NS, "attach-session", "-t", "s:on" -PassThru
    $attached = $false
    for ($i = 0; $i -lt 40; $i++) {
        if ((P list-clients -t s -F '#{client_pid}') -match '\d') { $attached = $true; break }
        Start-Sleep -Milliseconds 250
    }
    if ($attached) { Write-Pass "a real client is attached" } else { Write-Fail "no client attached" }
    Check "with a client attached, the active pane reports 1" (P display -p -t "s:on.0" '#{bracket_paste_flag}') "1"
    P select-pane -t $p2 | Out-Null
    Check "list-panes is unchanged by the client and select-pane" (P list-panes -t 's:on' -F '#{pane_id}=#{bracket_paste_flag}') "$p1=1`n$p2=0"
    P copy-mode -t $p1 | Out-Null
    Start-Sleep -Milliseconds 300
    # tmux reads the mode's screen while a pane is in copy mode, and a paste
    # there is plain text, so the flag reads 0 until the mode ends.
    Check "in copy mode it reads 0, as in tmux" (P display -p -t $p1 '#{pane_in_mode}:#{bracket_paste_flag}') "1:0"
    P send-keys -t $p1 -X cancel | Out-Null
    Start-Sleep -Milliseconds 300
    Check "after copy mode it reads 1 again" (P display -p -t $p1 '#{pane_in_mode}:#{bracket_paste_flag}') "0:1"
}
catch {
    Write-Fail "exception: $_"
}
finally {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    if ($client) { try { Stop-Process -Id $client.Id -Force -EA Stop } catch {} }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $ROOT -EA SilentlyContinue
    Remove-Item Env:PSMUX_DATA_DIR, Env:PSMUX_NO_WARM -EA SilentlyContinue
    if ($null -ne $savedShell) { $env:SHELL = $savedShell }
}

$defaultAfter = (& $PSMUX ls 2>&1 | Out-String).Trim()
if ($defaultAfter -ne $defaultBefore) {
    Write-Host "  [WARN] the default namespace changed: '$defaultBefore' -> '$defaultAfter'" -ForegroundColor Yellow
}
Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
