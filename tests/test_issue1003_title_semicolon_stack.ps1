# Issue #1003: pane titles from OSC 0 and OSC 2 keep a `;`, and CSI 22 ; 0 t
# / CSI 23 ; 0 t save and restore the title, as tmux does. tmux takes
# everything after the first `;` as the title (input_exit_osc, input.c:
# 2714-2731 at tag 3.8), and keeps a stack of at most ten saved titles
# (input.c:2187-2209, screen.c:292-337). psmux dropped a title holding a `;`,
# and ignored the stack, so a program that saved the title, set its own and
# restored the old one on exit left its own title on the pane. RIS and
# send-keys -R keep the title and the saved ones in tmux, and in psmux now.
#
# Neither console host keeps saved titles itself, and both hand psmux the
# sequences. The inbox conhost passes CSI 22 / 23 t at once and holds a title
# for its next screen update, so what is written in one go reaches psmux in
# another order: `ESC[22;0t ESC[23;0t ESC]0;inner`. Written apart, as a program
# saves at its start and restores at its end, the order holds and the stack
# works there too, so the sep_ cases write each piece on its own.
#
# Each case is written as raw bytes by a program in a pane, then
# `#{pane_title}` is read. Every expectation is what tmux 3.8 (`-f /dev/null`,
# WSL) showed for the same bytes, with allow-set-title on as in tmux. A pane's title starts as the host name in
# both (window.c:1479-1480), so restoring the title saved before any was set
# gives the host name back.
#
# [1] the inbox conhost; [2] OpenConsole (PSMUX_CONPTY_DIR), when
# PSMUX_TEST_CONPTY_DIR names a folder holding conpty.dll, skipped otherwise.
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per server;
# cleanup is `-L <ns> kill-server`. PSMUX_TEST_BIN or PSMUX_EXE picks the
# binary (defaults to target\release\psmux.exe); PSMUX_CONPTY_DIR_FOR_TEST,
# which the sweep sets, names the OpenConsole folder as well.

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { $env:PSMUX_EXE }
$ConptyDir = if ($env:PSMUX_TEST_CONPTY_DIR) { $env:PSMUX_TEST_CONPTY_DIR } elseif ($env:PSMUX_CONPTY_DIR_FOR_TEST) { $env:PSMUX_CONPTY_DIR_FOR_TEST } else { '' }
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
# The panes run pwsh commands; SHELL would make psmux start them in bash.
Remove-Item Env:SHELL -EA SilentlyContinue
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkGray }
function Check($name, $got, $want) {
    if ("$got" -ceq "$want") { Write-Pass "$name = '$got'" } else { Write-Fail "$name expected '$want', got '$got'" }
}

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_tss_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force "$TempHome\p" | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
# psmux ships allow-set-title off (PowerShell 7 rewrites the title all the
# time, docs/pane-titles.md); tmux has it on. Programs set titles here.
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value "set -g allow-set-title on"

$E = [string][char]27
$BEL = [string][char]7
$ST = "$E\"
function Title($t) { "${E}]2;$t$BEL" }
$push = "${E}[22;0t"
$pop = "${E}[23;0t"
$cap = (-join (0..10 | ForEach-Object { (Title "t$_") + $push })) + (Title 't11') + ($pop * 11)
# name, the text written, the pane title tmux 3.8 showed ('HOST': the
# pane's first title, the host name), and for a save or restore written in
# one go the title on the inbox conhost route, where the order changes (see
# the top). $SEP splits the text into writes 700 ms apart.
$SEP = [string][char]1
$cases = @(
    # A4: everything after the first `;` is the title.
    @('osc0_semicolon',  "${E}]0;a;b$BEL", 'a;b'),
    @('osc2_semicolons', "${E}]2;x;y;z$ST", 'x;y;z'),
    @('trailing',        "${E}]2;x;$BEL", 'x;'),
    @('plain',           (Title 'plain'), 'plain'),
    # A5: save and restore, for both titles (0) or the window title (2).
    @('push_pop',        ((Title 'outer') + $push + (Title 'inner') + $pop), 'outer', 'inner'),
    @('push_pop_2',      ((Title 'outer') + "${E}[22;2t" + (Title 'inner') + "${E}[23;2t"), 'outer', 'inner'),
    # The icon name alone (1), or no second parameter: nothing.
    @('icon_only',       ((Title 'outer') + "${E}[22;1t" + (Title 'inner') + "${E}[23;1t"), 'inner'),
    @('no_second',       ((Title 'outer') + "${E}[22t" + (Title 'inner') + "${E}[23t"), 'inner'),
    # Nothing saved: the title stays.
    @('pop_empty',       ((Title 'only') + $pop), 'only'),
    # Ten saved at most, the oldest dropped.
    @('cap_ten',         $cap, 't1', 't11'),
    # Saved before any title was set: back to the pane's first title.
    @('first_title',     ($push + (Title 'app') + $pop), 'HOST', 'app'),
    # Written apart, as programs do: the same on both routes.
    @('sep_push_pop',    ((Title 'outer') + $SEP + $push + $SEP + (Title 'inner') + $SEP + $pop), 'outer'),
    @('sep_two_deep',    ((Title 'a') + $SEP + $push + $SEP + (Title 'b') + $SEP + $push + $SEP + (Title 'c') + $SEP + $pop), 'b'),
    @('sep_ris_pop',     ((Title 'outer') + $SEP + $push + $SEP + (Title 'inner') + $SEP + "${E}c" + $SEP + $pop), 'outer'),
    # RIS keeps the title and the saved ones (screen_write_reset touches
    # neither, input.c:1407-1411).
    @('ris_keeps',       ((Title 'kept') + "${E}c"), 'kept'),
    @('ris_then_pop',    ((Title 'outer') + $push + (Title 'inner') + "${E}c" + $pop), 'outer', 'inner'),
    @('ris_then_push',   ((Title 'a') + "${E}c" + $push + (Title 'b') + $pop), 'a', 'b')
)
foreach ($c in $cases) { [IO.File]::WriteAllBytes("$TempHome\p\$($c[0]).bin", [Text.Encoding]::UTF8.GetBytes($c[1])) }
Set-Content -Path "$TempHome\show.ps1" -Value @(
    'param([string]$Payload)'
    '[Console]::OutputEncoding = [Text.Encoding]::UTF8'
    '$o = [Console]::OpenStandardOutput()'
    '$pieces = [Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes($Payload)) -split [string][char]1'
    'foreach ($p in $pieces) { $b = [Text.Encoding]::UTF8.GetBytes($p); $o.Write($b, 0, $b.Length); $o.Flush(); if ($pieces.Count -gt 1) { Start-Sleep -Milliseconds 700 } }'
    'Set-Content -Path ($Payload + ".done") -Value 1'
    'Start-Sleep 600'
)
# send-keys -R between a save and a restore: the program writes each piece
# apart, waits for the suite to send -R, and then writes the restore.
foreach ($r in @(@('r1', (Title 'outer')), @('r2', $push), @('r3', (Title 'inner')), @('r4', $pop))) {
    [IO.File]::WriteAllBytes("$TempHome\$($r[0]).bin", [Text.Encoding]::UTF8.GetBytes($r[1]))
}
Set-Content -Path "$TempHome\show2.ps1" -Value @(
    'param([string]$Dir)'
    '[Console]::OutputEncoding = [Text.Encoding]::UTF8'
    '$o = [Console]::OpenStandardOutput()'
    'foreach ($n in "r1", "r2", "r3") { $b = [IO.File]::ReadAllBytes("$Dir/$n.bin"); $o.Write($b, 0, $b.Length); $o.Flush(); Start-Sleep -Milliseconds 700 }'
    'Set-Content -Path "$Dir/r3.done" -Value 1'
    'while (-not (Test-Path "$Dir/go")) { Start-Sleep -Milliseconds 100 }'
    '$b = [IO.File]::ReadAllBytes("$Dir/r4.bin"); $o.Write($b, 0, $b.Length); $o.Flush()'
    'Set-Content -Path "$Dir/r4.done" -Value 1'
    'Start-Sleep 600'
)
$pwsh = (Get-Command pwsh).Source.Replace('\', '/')
$fwd = $TempHome.Replace('\', '/')
$script:NS = ""
function P { & $PSMUX -L $script:NS @args 2>&1 }

function Run-Cases($route) {
    Get-ChildItem "$TempHome\p\*.done" -EA SilentlyContinue | Remove-Item -Force
    $script:NS = "tss_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 60 -y 10 | Out-Null
    Start-Sleep -Milliseconds 2000
    # A pane's first title is the host name (the first window's shell may
    # set its own, so the name is read from #{host}).
    $hostTitle = "$(P display -p '#{host}')"
    Check "${route}: the host name is known" ($hostTitle -ne '') $true
    foreach ($c in $cases) {
        P new-window -d -t s -n $c[0] "& '$pwsh' -NoProfile -File $fwd/show.ps1 $fwd/p/$($c[0]).bin" | Out-Null
    }
    $deadline = (Get-Date).AddSeconds(40)
    while ((Get-Date) -lt $deadline -and @(Get-ChildItem "$TempHome\p\*.done" -EA SilentlyContinue).Count -lt $cases.Count) {
        Start-Sleep -Milliseconds 250
    }
    Start-Sleep -Milliseconds 1500
    # With no client attached, psmux copies a program's title to the pane
    # only when a client draws or a window list is asked for (its
    # propagate_osc_titles); tmux sets it as the OSC arrives. list-windows
    # copies them here, so the reads below see the program's titles.
    P list-windows | Out-Null
    foreach ($c in $cases) {
        $want = if ($route -eq 'inbox conhost' -and $c.Count -gt 3) { $c[3] } else { $c[2] }
        if ($want -ceq 'HOST') { $want = $hostTitle }
        Check "${route}: $($c[0]) pane_title" "$(P display -p -t "s:$($c[0])" '#{pane_title}')" $want
    }
    # send-keys -R is an RIS on the pane's parser, so the saved title stays:
    # tmux 3.8 shows `outer` after the restore.
    Remove-Item "$TempHome\r3.done", "$TempHome\r4.done", "$TempHome\go" -Force -EA SilentlyContinue
    P new-window -t s -n keys_r "& '$pwsh' -NoProfile -File $fwd/show2.ps1 $fwd" | Out-Null
    $deadline = (Get-Date).AddSeconds(20)
    while ((Get-Date) -lt $deadline -and -not (Test-Path "$TempHome\r3.done")) { Start-Sleep -Milliseconds 100 }
    Start-Sleep -Milliseconds 300
    P send-keys -R -t 's:keys_r' | Out-Null
    Set-Content -Path "$TempHome\go" -Value 1
    $deadline = (Get-Date).AddSeconds(20)
    while ((Get-Date) -lt $deadline -and -not (Test-Path "$TempHome\r4.done")) { Start-Sleep -Milliseconds 100 }
    Start-Sleep -Milliseconds 800
    P list-windows | Out-Null
    Check "${route}: send-keys -R keeps the saved title" "$(P display -p -t 's:keys_r' '#{pane_title}')" 'outer'
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

Write-Host "`n=== pane titles: a ; in OSC 0 / 2, and the title stack ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((& $PSMUX -V) -join ' ')"

try {
    Write-Host "`n[1] the inbox conhost" -ForegroundColor Yellow
    Run-Cases 'inbox conhost'

    Write-Host "`n[2] OpenConsole" -ForegroundColor Yellow
    if ($ConptyDir -and (Test-Path (Join-Path $ConptyDir 'conpty.dll'))) {
        $env:PSMUX_CONPTY_DIR = $ConptyDir
        Run-Cases 'OpenConsole'
        Remove-Item Env:PSMUX_CONPTY_DIR
    } else {
        Write-Info "skipped: set PSMUX_TEST_CONPTY_DIR to a folder holding conpty.dll and OpenConsole.exe"
    }
}
catch {
    Write-Fail "exception: $_"
}
finally {
    if ($script:NS) { P kill-server | Out-Null }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
