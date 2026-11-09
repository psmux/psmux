# allow-passthrough at runtime: off drops a DCS tmux; passthrough unparsed,
# as tmux does (input.c:2672-2674); on and all keep psmux's old reading,
# where the parser acts on the wrapped sequence, so a wrapped OSC 52 still
# copies (an opt in for tools that only write the wrapped form). A change
# with set -g must apply to a pane that already exists.
#
# One live pane runs a writer per step that writes a wrapped OSC 52 of a tag
# (closing ST kept or left off, the inbox conhost form) and then a marker.
# With set-clipboard on a paste buffer holding the tag means the parser acted
# on the wrapped copy. Steps: off, on, off, all, then set -u (back to off).
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
        @('on', 'on', $true), @('on', $null, $true, 'lost'),
        @('off', 'off', $false), @('all', 'all', $true), @('unset', '-u', $false, 'lost')
    )
    foreach ($s in $steps) {
        $step++
        $label = $s[0]; $set = $s[1]; $want = $s[2]; $form = if ($s.Count -gt 3) { $s[3] } else { 'st' }
        if ($set -eq '-u') { & $PSMUX -L $ns set -gu allow-passthrough }
        elseif ($set) { & $PSMUX -L $ns set -g allow-passthrough $set }
        $shown = (& $PSMUX -L $ns show -gv allow-passthrough | Out-String).Trim()
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
