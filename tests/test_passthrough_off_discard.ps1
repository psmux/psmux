# allow-passthrough off: a DCS tmux; passthrough is dropped whole, as tmux
# does (input_dcs_dispatch, input.c:2672-2674), so the pane parser does not
# act on what it wraps. Before, vte ended the DCS at the first doubled ESC and
# parsed the inner OSC 52, which made a paste buffer from a wrapped copy.
#
# A pane writes only WRAPPED OSC 52 copies (BEL and ST terminated, one
# followed by text), each followed by visible text, plus a sixel and a
# DECRQSS. With set-clipboard on, a paste buffer holding WRAPPEDn means the
# parser acted on a wrapped copy. Every VISn must still be on screen: under
# the inbox conhost the closing ST is lost (#1026) and the parser must not
# swallow what follows.
#
# The Windows 10 inbox conhost (build 19045) unwraps the passthrough before
# psmux reads the pane: it drops ESC P and the closing ST, prints "tmux;" as
# text, hands psmux an inner sequence that ends with BEL (or a CSI) ahead of
# the text, and drops one that ends with ST. psmux never sees the wrapper
# there, so on a build before 22000 without PSMUX_CONPTY_DIR the BEL ended
# copies (WRAPPED1 and WRAPPED3) make paste buffers as unwrapped ones do,
# WRAPPED2 makes none, and "tmux;" may show; the OSC 52 itself must still
# never show, and every VISn must. OpenConsole passes the wrapper on, so
# with PSMUX_CONPTY_DIR the checks are as on Windows 11.
#
# Runs under a private -L namespace and PSMUX_DATA_DIR; cleans up with
# `psmux -L <ns> kill-server` only. PSMUX_EXE picks another binary;
# PSMUX_CONPTY_DIR runs the pane on OpenConsole.

$ErrorActionPreference = 'Continue'
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

Write-Host "psmux: $PSMUX ($(& $PSMUX -V))"
$Win10Inbox = [Environment]::OSVersion.Version.Build -lt 22000 -and -not $env:PSMUX_CONPTY_DIR
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_ptoff_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $work | Out-Null
$ns = 'ptoff_' + [guid]::NewGuid().ToString('N').Substring(0, 6)
$savedDataDir = $env:PSMUX_DATA_DIR
$env:PSMUX_DATA_DIR = $work
Remove-Item Env:SHELL, Env:TMUX, Env:TMUX_PANE -ErrorAction SilentlyContinue

$writer = Join-Path $work 'writer.ps1'
@'
$o = [Console]::OpenStandardOutput()
function W([string]$s) { $b = [System.Text.Encoding]::ASCII.GetBytes($s); $o.Write($b, 0, $b.Length); $o.Flush() }
$E = [char]27; $BEL = [char]7; $ST = "$E\"
function Wrap([string]$inner) { "${E}Ptmux;" + $inner.Replace("$E", "$E$E") + $ST }
$cases = @(
    @('osc52', ((Wrap "$E]52;c;V1JBUFBFRDE=$BEL") + 'VIS1')),
    @('osc52st', ((Wrap "$E]52;c;V1JBUFBFRDI=$ST") + 'VIS2')),
    @('csi', ((Wrap "$E[6 q") + "$E[1mVIS3$E[m")),
    @('text', ((Wrap "$E]52;c;V1JBUFBFRDM=$BEL") + 'VIS4')),
    @('sixel', ("${E}Pq#0;2;0;0;0#0~~~~$ST" + 'VIS5')),
    @('dcsq', ("${E}P`$qm$ST" + 'VIS6'))
)
Start-Sleep -Seconds 3
foreach ($c in $cases) {
    W "`r`n<<$($c[0])>>"; Start-Sleep -Milliseconds 150
    W $c[1]; Start-Sleep -Milliseconds 150
    W '<<end>>'; Start-Sleep -Milliseconds 200
}
W "`r`n<<done>>`r`n"
Start-Sleep -Seconds 60
'@ | Set-Content -Encoding ascii $writer

$shell = (Get-Command pwsh -EA SilentlyContinue).Source
if (-not $shell) { $shell = (Get-Command powershell -EA Stop).Source }

try {
    $conf = Join-Path $work 'c.conf'
    Set-Content -Path $conf -Value "set -g allow-passthrough off`nset -g set-clipboard on"
    & $PSMUX -L $ns -f $conf new-session -d -s s -x 120 -y 40 "`"$shell`" -NoProfile -NoLogo -File `"$writer`""
    $cap = ''
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline) {
        $cap = (& $PSMUX -L $ns capture-pane -p -t s | Out-String)
        if ($cap -match '<<done>>') { break }
        Start-Sleep -Milliseconds 300
    }
    Start-Sleep -Milliseconds 500
    $cap = (& $PSMUX -L $ns capture-pane -p -t s | Out-String)
    $bufs = (& $PSMUX -L $ns list-buffers -F '#{buffer_sample}' | Out-String)
    if ($cap -match '<<done>>') { Write-Pass 'pane program ran to the end' } else { Write-Fail "no <<done>> on screen: $cap" }
    $acted = [regex]::Matches($bufs, 'WRAPPED\d') | ForEach-Object { $_.Value }
    if ($Win10Inbox) {
        # The Windows 10 inbox conhost hands psmux the BEL ended copies unwrapped.
        $got = (@($acted) | Sort-Object) -join ', '
        if ($got -eq 'WRAPPED1, WRAPPED3') { Write-Pass 'the copies the Windows 10 inbox conhost unwrapped made paste buffers, the ST ended one none' }
        else { Write-Fail "paste buffers from the wrapped OSC 52: [$got], want [WRAPPED1, WRAPPED3] on the Windows 10 inbox conhost" }
    } elseif (-not $acted) { Write-Pass 'no wrapped OSC 52 made a paste buffer' }
    else { Write-Fail "wrapped OSC 52 acted on with allow-passthrough off: $($acted -join ', ')" }
    foreach ($n in 1..6) {
        if ($cap -match "VIS$n") { Write-Pass "VIS$n visible after its sequence" } else { Write-Fail "VIS$n missing: the text after the sequence was swallowed" }
    }
    if ($Win10Inbox) {
        # That host prints the wrapper's "tmux;" itself.
        if ($cap -match '\]52|V1JBUFBF') { Write-Fail "OSC 52 bytes on screen: $cap" } else { Write-Pass 'no OSC 52 bytes on screen' }
    } elseif ($cap -match 'mux;|\]52|V1JBUFBF') { Write-Fail "wrapper bytes on screen: $cap" } else { Write-Pass 'no wrapper bytes on screen' }
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
