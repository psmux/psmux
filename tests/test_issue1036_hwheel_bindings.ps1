# Issue #1036 follow up: the horizontal wheel as tmux's buttons 6 and 7.
#
# tmux 3.8 has no WheelLeft/WheelRight. SGR buttons 66 and 67 are
# MOUSE_BUTTON_6/7, so a sideways notch is the key MouseDown6Pane (left) or
# MouseDown7Pane (right). Neither has a default binding; unbound, the notch is
# forwarded to the pane only if its program enabled mouse reporting, and it
# never scrolls.
#
# Two input shapes reach the same client code:
#   SSH    the client reads SGR bytes on stdin (SSH_CONNECTION set). Driven by
#          a nested client inside an outer psmux pane, bytes sent with
#          `send-keys -l` (same harness as test_issue629_ssh_vt_wheel.ps1).
#   LOCAL  the client reads Win32 records from its console. Driven by a
#          conhost window hosting the client and MOUSE_HWHEELED records
#          written with tests\mouse_injector.cs (left/right).
#
# Checks per shape: bindings fire (plain and with a modifier), a plain shell
# gets nothing and never enters copy mode (#598 gate), copy mode does not
# move, a VT mouse app gets SGR 66/67, and on the local shape a native
# record reading app gets MOUSE_HWHEELED records while its vertical wheel
# record is unchanged.
#
# Every psmux call uses its own random -L namespaces and PSMUX_DATA_DIR.

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$Tests = Split-Path -Parent $MyInvocation.MyCommand.Path
$rand  = Get-Random -Maximum 999999
$ESC   = [char]27

$ownDataDir = $false
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path $env:TEMP "psmux_i1036hw_$rand"
    New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
    $ownDataDir = $true
}
$dd = $env:PSMUX_DATA_DIR

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

# Helpers compiled once per run into the data dir.
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$echo = Join-Path $dd "mouse_echo_child.exe"
$nat  = Join-Path $dd "native_mouse_child.exe"
$inj  = Join-Path $dd "mouse_injector.exe"
& $csc /nologo /out:$echo (Join-Path $Tests "mouse_echo_child.cs") | Out-Null
& $csc /nologo /out:$nat  (Join-Path $Tests "native_mouse_child.cs") | Out-Null
& $csc /nologo /out:$inj  (Join-Path $Tests "mouse_injector.cs") | Out-Null
$natLog = Join-Path $env:TEMP "psmux_native_mouse.txt"

function EchoRecv([string]$log) {
    if (-not (Test-Path $log)) { return "" }
    (Get-Content $log | Where-Object { $_ -like 'RECV*' }) -join ' '
}

# ---------------------------------------------------------------------
# Shape-independent checks. $send: scriptblock taking 'left','right','up'
# and a modifier mask; $ns: the namespace that owns the pane; $t: target.
# ---------------------------------------------------------------------
function Run-Checks([string]$shape, [string]$ns, [string]$t, [scriptblock]$send, [string]$echoLog, [bool]$native) {
    $F = { param($f) (& $PSMUX -L $ns display -t $t -p $f | Out-String).Trim() }

    Write-Host "`n[$shape 1] bound MouseDown6Pane / MouseDown7Pane fire" -ForegroundColor Yellow
    & $PSMUX -L $ns bind -n MouseDown6Pane set -g '@hw6' fired | Out-Null
    & $PSMUX -L $ns bind -n MouseDown7Pane set -g '@hw7' fired | Out-Null
    & $send 'left' 0; & $send 'right' 0; Start-Sleep -Milliseconds 900
    $a = & $F '#{@hw6}'; $b = & $F '#{@hw7}'
    if ($a -eq 'fired' -and $b -eq 'fired') { Write-Pass "left ran MouseDown6Pane, right ran MouseDown7Pane" }
    else { Write-Fail "bindings did not fire (@hw6='$a' @hw7='$b')" }
    & $PSMUX -L $ns unbind -n MouseDown6Pane | Out-Null
    & $PSMUX -L $ns unbind -n MouseDown7Pane | Out-Null

    if ($shape -eq 'SSH') {
        # Meta rides in the SGR button (67 + 8), so M-MouseDown7Pane is reachable.
        & $PSMUX -L $ns bind -n M-MouseDown7Pane set -g '@hwm' fired | Out-Null
        & $send 'right' 8; Start-Sleep -Milliseconds 900
        $m = & $F '#{@hwm}'
        if ($m -eq 'fired') { Write-Pass "Meta + right ran M-MouseDown7Pane" }
        else { Write-Fail "M-MouseDown7Pane did not fire (@hwm='$m')" }
        & $PSMUX -L $ns unbind -n M-MouseDown7Pane | Out-Null
    }

    Write-Host "`n[$shape 2] unbound over a plain shell: nothing at all" -ForegroundColor Yellow
    & $send 'left' 0; & $send 'right' 0; & $send 'left' 0; & $send 'right' 0
    Start-Sleep -Milliseconds 900
    $cap = (& $PSMUX -L $ns capture-pane -t $t -p | Out-String)
    $mode = & $F '#{pane_in_mode}'
    if ($mode -eq '0') { Write-Pass "no copy mode" } else { Write-Fail "pane_in_mode=$mode" }
    if ($cap -notmatch '<6[67];|\[<6[67]') { Write-Pass "no SGR text typed into the shell (#598 gate)" }
    else { Write-Fail "SGR 66/67 leaked into the shell prompt" }

    Write-Host "`n[$shape 3] copy mode does not move" -ForegroundColor Yellow
    & $send 'up' 0; & $send 'up' 0; & $send 'up' 0; Start-Sleep -Milliseconds 900
    $p0 = & $F '#{scroll_position}'
    & $send 'left' 0; & $send 'right' 0; & $send 'left' 0; Start-Sleep -Milliseconds 900
    $p1 = & $F '#{scroll_position}'
    $mode = & $F '#{pane_in_mode}'
    if ($mode -eq '1' -and $p0 -match '^\d+$' -and [int]$p0 -gt 0 -and $p1 -eq $p0) { Write-Pass "scroll_position stayed $p0" }
    else { Write-Fail "copy mode moved or was not entered (mode=$mode pos $p0 -> $p1)" }
    & $PSMUX -L $ns send-keys -t $t -X cancel | Out-Null; Start-Sleep -Milliseconds 500

    Write-Host "`n[$shape 4] a VT mouse app gets SGR 66/67" -ForegroundColor Yellow
    & $PSMUX -L $ns respawn-pane -k -t $t $echo | Out-Null; Start-Sleep -Seconds 4
    & $send 'left' 0; & $send 'right' 0; & $send 'up' 0; Start-Sleep -Milliseconds 1200
    $recv = EchoRecv $echoLog
    if ($recv -match '\[<66;\d+;\d+M' -and $recv -match '\[<67;\d+;\d+M') { Write-Pass "app received <66 and <67" }
    else { Write-Fail "app did not get the horizontal reports: $recv" }
    if ($recv -match '\[<64;\d+;\d+M') { Write-Pass "vertical wheel still reaches the app" }
    else { Write-Fail "vertical wheel report missing: $recv" }

    if ($native) {
        Write-Host "`n[$shape 5] a native record app gets MOUSE_HWHEELED" -ForegroundColor Yellow
        Remove-Item $natLog -Force -EA SilentlyContinue
        & $PSMUX -L $ns respawn-pane -k -t $t $nat | Out-Null; Start-Sleep -Seconds 4
        & $send 'left' 0; & $send 'right' 0; & $send 'up' 0; Start-Sleep -Milliseconds 1200
        $lines = if (Test-Path $natLog) { Get-Content $natLog | Where-Object { $_ -match '^MOUSE' } } else { @() }
        $hl = $lines | Where-Object { $_ -match 'flags=0x8 wheel=-120' }
        $hr = $lines | Where-Object { $_ -match 'flags=0x8 wheel=120' }
        $v  = $lines | Where-Object { $_ -match 'flags=0x4 wheel=1\d\d' }
        $h4 = $lines | Where-Object { $_ -match 'flags=0x4 wheel=-' }
        if ($hl -and $hr) { Write-Pass "left and right arrived as MOUSE_HWHEELED (-120 / +120)" }
        else { Write-Fail "no MOUSE_HWHEELED records: $($lines -join ' / ')" }
        if ($v -and -not $h4) { Write-Pass "vertical up still a MOUSE_WHEELED record, and no sideways notch became a vertical one" }
        else { Write-Fail "vertical record wrong: $($lines -join ' / ')" }
    }
}

# =====================================================================
# SSH shape
# =====================================================================
Write-Host "`n=== #1036 horizontal wheel: SSH input shape ===" -ForegroundColor Cyan
$NS_OUT = "i1036hwo$rand"; $NS_IN = "i1036hwi$rand"
$echoLogS = Join-Path $dd "echo_ssh.txt"
& $PSMUX -L $NS_OUT new-session -d -s outer -x 120 -y 40 | Out-Null
Start-Sleep -Seconds 4
$cmd = "Remove-Item Env:\PSMUX_SESSION,Env:\PSMUX_SESSION_NAME,Env:\PSMUX_PANE -EA SilentlyContinue; " +
       "`$env:SSH_CONNECTION='10.0.0.5 51000 10.0.0.9 22'; `$env:SSH_TTY='/dev/pts/0'; `$env:TERM='xterm-256color'; " +
       "`$env:PSMUX_MOUSE_ECHO_LOG='$echoLogS'; `$env:PSMUX_DATA_DIR='$dd'; & '$PSMUX' -L $NS_IN new-session -s inner"
& $PSMUX -L $NS_OUT send-keys -t outer $cmd Enter | Out-Null
$up = $false
for ($i = 0; $i -lt 30; $i++) {
    Start-Sleep -Milliseconds 500
    & $PSMUX -L $NS_IN has-session -t inner 2>$null
    if ($LASTEXITCODE -eq 0) { $up = $true; break }
}
if (-not $up) {
    Write-Fail "nested SSH shaped client did not start"
} else {
    Start-Sleep -Seconds 3
    & $PSMUX -L $NS_IN set -g mouse on | Out-Null
    & $PSMUX -L $NS_IN send-keys -t inner '1..300 | % { "SB_$_" }' Enter | Out-Null
    Start-Sleep -Seconds 4
    $sendSsh = {
        param($dir, $mods)
        $b = switch ($dir) { 'up' { 64 } 'left' { 66 } 'right' { 67 } }
        & $PSMUX -L $NS_OUT send-keys -t outer -l "$ESC[<$($b + $mods);30;10M" | Out-Null
        Start-Sleep -Milliseconds 300
    }
    Run-Checks 'SSH' $NS_IN 'inner' $sendSsh $echoLogS $false
}
& $PSMUX -L $NS_IN kill-server 2>&1 | Out-Null
& $PSMUX -L $NS_OUT kill-server 2>&1 | Out-Null

# =====================================================================
# LOCAL console shape
# =====================================================================
Write-Host "`n=== #1036 horizontal wheel: local console shape ===" -ForegroundColor Cyan
$NS_L = "i1036hwl$rand"
$echoLogL = Join-Path $dd "echo_local.txt"
$env:PSMUX_MOUSE_ECHO_LOG = $echoLogL
& $PSMUX -L $NS_L new-session -d -s w -x 100 -y 30 | Out-Null
Start-Sleep -Seconds 3
& $PSMUX -L $NS_L set -g mouse on | Out-Null
& $PSMUX -L $NS_L send-keys -t w '1..300 | % { "SB_$_" }' Enter | Out-Null
$proc = Start-Process -FilePath "$env:WINDIR\System32\conhost.exe" -ArgumentList "`"$PSMUX`"","-L",$NS_L,"attach","-t","w" -PassThru
try {
    Start-Sleep -Seconds 4
    $child = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($proc.Id)" | Where-Object { $_.Name -eq 'psmux.exe' } | Select-Object -First 1
    if (-not $child) {
        Write-Fail "local client did not start under conhost"
    } else {
        $script:cpid = [int]$child.ProcessId
        $sendLocal = {
            param($dir, $mods)
            & $inj $script:cpid $dir 1 20 8 | Out-Null
            Start-Sleep -Milliseconds 400
        }
        Run-Checks 'LOCAL' $NS_L 'w' $sendLocal $echoLogL $true
    }
} finally {
    & $PSMUX -L $NS_L kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue }
}

if ($ownDataDir) { Remove-Item $dd -Recurse -Force -EA SilentlyContinue }
Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)"
Write-Host "  Failed: $($script:TestsFailed)"
exit $(if ($script:TestsFailed -gt 0) { 1 } else { 0 })
