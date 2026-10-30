# Issue #934: per pane @mouse-force on must route a wheel pane-scroll to a
# primary screen raw stdin application (no alternate screen, no mouse DECSET)
# as a wheel report, instead of entering psmux copy mode.
# Controls: explicit @mouse-force off, and an unset option, must keep the
# default (tmux) behaviour: the wheel enters copy mode and the app gets nothing.
#
# Fully isolated: private -L namespace and private PSMUX_DATA_DIR. Never touches
# the default namespace; cleans up only its own namespace.
#
# Usage: pwsh tests\test_issue934_mouse_force_wheel.ps1 [-Bin path\to\psmux.exe]

param([string]$Bin = $env:PSMUX_TEST_BIN)

$ErrorActionPreference = 'Continue'
if (-not $Bin) {
    $Bin = Join-Path $PSScriptRoot '..\target\release\psmux.exe'
}
$Bin = (Resolve-Path $Bin).Path
$node = (Get-Command node -ErrorAction SilentlyContinue).Source
if (-not $node) { Write-Host 'SKIP: node is not installed'; exit 0 }

$script:pass = 0; $script:fail = 0
function Check([bool]$ok, [string]$msg) {
    if ($ok) { $script:pass++; Write-Host "  [PASS] $msg" -ForegroundColor Green }
    else     { $script:fail++; Write-Host "  [FAIL] $msg" -ForegroundColor Red }
}

$ns   = "t934_$PID"
$work = Join-Path ([IO.Path]::GetTempPath()) "psmux_t934_$PID"
New-Item -ItemType Directory -Force $work | Out-Null
$dataDir = Join-Path $work 'data'
New-Item -ItemType Directory -Force $dataDir | Out-Null
$prevDataDir = $env:PSMUX_DATA_DIR
$defaultBefore = (& $Bin ls 2>&1) -join ' '
$env:PSMUX_DATA_DIR = $dataDir

$probe = Join-Path $work 'wheel-probe.js'
@'
const fs = require('fs');
process.stdin.setRawMode(true);
process.stdout.write('raw stdin probe ready\r\n');
// Deliberately no alternate screen or mouse DECSET: @mouse-force is the opt in.
process.stdin.on('data', data =>
  fs.appendFileSync(process.argv[2], JSON.stringify({
    hex: data.toString('hex'), text: data.toString()
  }) + '\n')
);
setInterval(() => {}, 1000);
'@ | Set-Content -Encoding ascii $probe

function Send-Tcp([string]$session, [string]$line) {
    $base = Join-Path $dataDir "$($ns)__$session"
    $port = [int](Get-Content "$base.port" -Raw)
    $key  = (Get-Content "$base.key" -Raw).Trim()
    $c = [Net.Sockets.TcpClient]::new('127.0.0.1', $port)
    try {
        $s = $c.GetStream(); $s.ReadTimeout = 2000
        $b = [Text.Encoding]::UTF8.GetBytes("AUTH $key`n$line`n")
        $s.Write($b, 0, $b.Length)
        $buf = New-Object byte[] 4096
        try { [void]$s.Read($buf, 0, $buf.Length) } catch {}
    } finally { $c.Dispose() }
}

function Wait-Ready([string]$session, [string]$log) {
    for ($i = 0; $i -lt 100; $i++) {
        $cap = & $Bin -L $ns capture-pane -p -t $session 2>$null
        if (($cap -join "`n") -match 'raw stdin probe ready') { return $true }
        Start-Sleep -Milliseconds 100
    }
    return $false
}

function Layout([string]$session) {
    $d = & $Bin -L $ns -t $session dump-state 2>$null | ConvertFrom-Json
    return $d.layout
}

# Runs one case. $force is 'on', 'off' or '' (option left unset).
# $cmd is the wheel request; {P} is replaced by the pane's numeric id.
# 'pane-scroll' is what a current client sends (handle_pane_scroll);
# 'scroll-up X Y' is the coordinate form (remote_scroll_wheel).
function Run-Case([string]$session, [string]$force, [string]$cmd = 'pane-scroll {P} up 10 5', [string[]]$bind = @()) {
    $log = Join-Path $work "$session.jsonl"
    Remove-Item $log -ErrorAction SilentlyContinue
    & $Bin -L $ns new-session -d -s $session -x 80 -y 24 -- $node $probe $log
    if (-not (Wait-Ready $session $log)) { Check $false "$session probe started"; return $null }
    if ($force) { & $Bin -L $ns -t $session set-option -p '@mouse-force' $force }
    if ($bind.Count) { & $Bin -L $ns -t $session bind-key @bind; Write-Host ("    keys: " + ((& $Bin -L $ns -t $session list-keys -T root 2>$null | Select-String WheelUpPane) -join " ")) }
    $opt = (& $Bin -L $ns -t $session show-options -p 2>$null) -join ' '
    $before = Layout $session
    $pid_ = ((& $Bin -L $ns display-message -p -t $session '#{pane_id}') -join '').Trim().TrimStart('%')
    Send-Tcp $session ($cmd -replace '\{P\}', $pid_)
    Start-Sleep -Milliseconds 500
    $after = Layout $session
    $got = if (Test-Path $log) { (Get-Content $log -Raw) } else { '' }
    Write-Host ("  [{0}] force='{1}' options='{2}'" -f $session, $force, $opt)
    Write-Host ("    before: alt={0} wants_mouse={1} copy_mode={2}" -f $before.alternate_screen, $before.wants_mouse, $before.copy_mode)
    Write-Host ("    after:  alt={0} wants_mouse={1} copy_mode={2} scroll_offset={3}" -f $after.alternate_screen, $after.wants_mouse, $after.copy_mode, $after.scroll_offset)
    Write-Host ("    probe:  {0}" -f ($(if ($got) { $got.Trim() } else { '<no input event>' })))
    return [pscustomobject]@{ Before = $before; After = $after; Input = $got }
}

try {
    Write-Host "=== Issue #934: @mouse-force wheel routing (ns=$ns) ==="

    Write-Host "`n--- Case 1: @mouse-force on, primary screen raw stdin app ---"
    $r = Run-Case 'forced' 'on'
    if ($r) {
        Check (-not $r.Before.alternate_screen -and -not $r.Before.wants_mouse) 'probe is a primary screen app with no mouse protocol'
        Check (-not $r.After.copy_mode) 'wheel did NOT enter copy mode'
        # SGR wheel up at pane (10,5) 0 based => ESC [ < 64 ; 11 ; 6 M
        Check ($r.Input -match '1b5b3c36343b31313b364d') 'probe received the SGR wheel up report ESC[<64;11;6M'
    }

    Write-Host "`n--- Case 2: @mouse-force off (explicit), same app ---"
    $r = Run-Case 'forcedoff' 'off'
    if ($r) {
        Check ([bool]$r.After.copy_mode) 'wheel entered copy mode (default behaviour kept)'
        Check (-not $r.Input) 'probe received no input'
    }

    Write-Host "`n--- Case 3: @mouse-force unset, same app ---"
    $r = Run-Case 'unforced' ''
    if ($r) {
        Check ([bool]$r.After.copy_mode) 'wheel entered copy mode (default behaviour kept)'
        Check (-not $r.Input) 'probe received no input'
    }

    Write-Host "`n--- Case 4: @mouse-force on, coordinate form scroll-up (remote_scroll_wheel) ---"
    $r = Run-Case 'forcedxy' 'on' 'scroll-up 10 5'
    if ($r) {
        Check (-not $r.After.copy_mode) 'scroll-up did NOT enter copy mode'
        Check ($r.Input -match '1b5b3c3634') 'probe received an SGR wheel up report (ESC[<64;...M)'
    }

    Write-Host "`n--- Case 5: @mouse-force unset, coordinate form scroll-up ---"
    $r = Run-Case 'unforcedxy' '' 'scroll-up 10 5'
    if ($r) {
        Check ([bool]$r.After.copy_mode) 'scroll-up entered copy mode (default behaviour kept)'
        Check (-not $r.Input) 'probe received no input'
    }

    Write-Host "`n--- Case 6: @mouse-force on, user binding WheelUpPane send-keys -M (send_mouse_event) ---"
    $r = Run-Case 'forcedbind' 'on' 'pane-scroll {P} up 10 5' @('-n', 'WheelUpPane', 'send-keys', '-M')
    if ($r) {
        Check (-not $r.After.copy_mode) 'send-keys -M did NOT enter copy mode'
        Check ($r.Input -match '1b5b3c36343b31313b364d') 'probe received the SGR wheel up report ESC[<64;11;6M'
    }

    Write-Host "`n--- Case 7: @mouse-force unset, user binding WheelUpPane send-keys -M ---"
    $r = Run-Case 'unforcedbind' '' 'pane-scroll {P} up 10 5' @('-n', 'WheelUpPane', 'send-keys', '-M')
    if ($r) {
        Check (-not $r.After.copy_mode) 'send-keys -M does not enter copy mode'
        Check (-not $r.Input) 'probe received no input (tmux input_key_mouse drops it)'
    }
} finally {
    & $Bin -L $ns kill-server 2>$null | Out-Null
    $env:PSMUX_DATA_DIR = $prevDataDir
    $defaultAfter = (& $Bin ls 2>&1) -join ' '
    if ($defaultAfter -ne $defaultBefore) {
        Write-Host "WARNING: default namespace changed: '$defaultBefore' -> '$defaultAfter'" -ForegroundColor Yellow
    }
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

Write-Host "`nResults: $($script:pass) passed, $($script:fail) failed"
if ($script:fail -gt 0) { exit 1 } else { exit 0 }
