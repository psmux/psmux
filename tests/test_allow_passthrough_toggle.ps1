# allow-passthrough at runtime. As in tmux, the pane parser never acts on
# the body of a DCS tmux; passthrough in any mode: off drops it
# (input.c:2672-2674), on and all write it raw to the attached clients'
# outer terminals (screen_write_rawstring), which
# tests/test_issue1068_passthrough_notify.ps1 checks byte for byte. Before
# PR #1068 psmux parsed the body with on/all, so a wrapped OSC 52 copied
# into a paste buffer; it no longer does. A change with set -g must apply
# to a pane that already exists, and no form may swallow the output after it.
#
# One live pane runs a writer per step that writes a wrapped OSC 52 of a tag
# (closing ST kept or left off, the inbox conhost form) and then a marker.
# With set-clipboard on a paste buffer holding the tag means the parser acted
# on the wrapped copy, which must never happen. Steps: off, on, off, all, then set -u (back to off).
#
# Runs under a private -L namespace and PSMUX_DATA_DIR; cleans up with
# `psmux -L <ns> kill-server` only. PSMUX_EXE picks another binary.

$ErrorActionPreference = 'Continue'
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

Write-Host "psmux: $PSMUX ($(& $PSMUX -V))"
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_pttog_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $work | Out-Null
$ns = 'pttog_' + [guid]::NewGuid().ToString('N').Substring(0, 6)
$savedDataDir = $env:PSMUX_DATA_DIR
$env:PSMUX_DATA_DIR = $work
Remove-Item Env:SHELL, Env:TMUX, Env:TMUX_PANE -ErrorAction SilentlyContinue

$writer = Join-Path $work 'w.ps1'
@'
param([string]$Tag, [string]$Form)
$o = [Console]::OpenStandardOutput()
function W([string]$s) { $b = [System.Text.Encoding]::ASCII.GetBytes($s); $o.Write($b, 0, $b.Length); $o.Flush() }
$E = [char]27; $BEL = [char]7
$b64 = [Convert]::ToBase64String([System.Text.Encoding]::ASCII.GetBytes($Tag))
$seq = "${E}Ptmux;$E$E]52;c;$b64$BEL"
if ($Form -eq 'st') { $seq += "$E\" }
W $seq
Start-Sleep -Milliseconds 100
W "$E[1mVIS-$Tag$E[m`r`n"
'@ | Set-Content -Encoding ascii $writer

$shell = (Get-Command pwsh -EA SilentlyContinue).Source
if (-not $shell) { $shell = (Get-Command powershell -EA Stop).Source }

function Wait-Screen([string]$text, [int]$ms) {
    $deadline = (Get-Date).AddMilliseconds($ms)
    while ((Get-Date) -lt $deadline) {
        if ((& $PSMUX -L $ns capture-pane -p -t s | Out-String).Contains($text)) { return $true }
        Start-Sleep -Milliseconds 200
    }
    return $false
}

try {
    $conf = Join-Path $work 'c.conf'
    Set-Content -Path $conf -Value "set -g set-clipboard on"
    & $PSMUX -L $ns -f $conf new-session -d -s s -x 120 -y 40 "`"$shell`" -NoProfile -NoLogo"
    if (-not (Wait-Screen '>' 15000)) { Write-Fail 'pane shell prompt never showed' }
    $step = 0
    $steps = @(
        @('off', $null, $false), @('off', $null, $false, 'lost'),
        @('on', 'on', $false), @('on', $null, $false, 'lost'),
        @('off', 'off', $false), @('all', 'all', $false), @('unset', '-u', $false, 'lost')
    )
    foreach ($s in $steps) {
        $step++
        $label = $s[0]; $set = $s[1]; $want = $s[2]; $form = if ($s.Count -gt 3) { $s[3] } else { 'st' }
        if ($set -eq '-u') { & $PSMUX -L $ns set -gu allow-passthrough }
        elseif ($set) { & $PSMUX -L $ns set -g allow-passthrough $set }
        $shown = (& $PSMUX -L $ns show -gv allow-passthrough | Out-String).Trim()
        $wantShown = if ($set -eq '-u') { 'off' } elseif ($set) { $set } else { $null }
        if ($wantShown -and $shown -ne $wantShown) { Write-Fail "step ${step}: show -gv allow-passthrough = $shown, want $wantShown" }
        $tag = "TAG$step$($label.ToUpper())"
        & $PSMUX -L $ns send-keys -t s "& '$writer' $tag $form" Enter
        if (-not (Wait-Screen "VIS-$tag" 15000)) { Write-Fail "step ${step} ($label): marker VIS-$tag never showed (swallowed?)"; continue }
        Start-Sleep -Milliseconds 400
        $bufs = (& $PSMUX -L $ns list-buffers -F '#{buffer_sample}' | Out-String)
        $acted = $bufs.Contains($tag)
        if ($acted -eq $want) {
            Write-Pass "step ${step}: allow-passthrough $shown, $form form, wrapped OSC 52 acted on = $acted"
        } else {
            Write-Fail "step ${step}: allow-passthrough $shown, $form form, wrapped OSC 52 acted on = $acted, want $want"
        }
    }
} catch {
    Write-Fail "harness error: $_"
} finally {
    & $PSMUX -L $ns kill-server 2>$null | Out-Null
    $env:PSMUX_DATA_DIR = $savedDataDir
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

Write-Host ""
Write-Host "Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed"
if ($script:TestsFailed -gt 0) { exit 1 }
exit 0
