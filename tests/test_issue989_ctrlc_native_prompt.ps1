# Issue #989: repeated Ctrl+C in a Git Bash pane killed the psmux server.
#
# With a native program on the pane console (Git Bash always has one: its
# bin\bash.exe launcher, and a PROMPT_COMMAND that runs a native exe makes it
# certain), the pane runs with ENABLE_PROCESSED_INPUT on, so conhost turns
# each typed 0x03 into a CTRL_C_EVENT that it dispatches on its own schedule
# to every process attached to the console.  psmux briefly attaches to the pane
# console for each Ctrl+C (and for mouse and key injections); when the event
# landed in that window, the server ran its default handler and exited with
# 0xC000013A (STATUS_CONTROL_C_EXIT), and every session was gone.
#
# tmux: Ctrl+C is a byte written to the pane; the server never sees a signal
# for it (server-client.c writes the key to the pane, the pty line discipline
# signals the pane's foreground process group only).
#
# A real psmux CLIENT is hosted under a pseudoconsole (conptyfeed610.cs) and
# the 0x03 bytes are written into its input, exactly the way a terminal sends
# a Ctrl+C keypress, so the real client keypath is exercised.  No keystroke
# reaches the desktop.  Every process this test starts is stopped by its PID,
# and every server lives in its own namespace and data dir.
#
# The test also proves Ctrl+C still interrupts what runs in the pane: an MSYS
# sleep and a native ping under Git Bash, Start-Sleep and ping under pwsh,
# ping under cmd.
#
# Usage: tests\test_issue989_ctrlc_native_prompt.ps1 [-Exe path\to\psmux.exe]
#        (or set PSMUX_TEST_BIN / PSMUX_EXE)

param([string]$Exe = $(if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { $env:PSMUX_EXE }))

$ErrorActionPreference = "Continue"
Remove-Item Env:SHELL -EA SilentlyContinue
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

if (-not $Exe) { $Exe = (Get-Command psmux -EA Stop).Source }
$PSMUX = (Resolve-Path $Exe).Path
$TESTS = $PSScriptRoot
$TAG = [guid]::NewGuid().ToString("N").Substring(0, 8)
$OUT = Join-Path $env:TEMP "psmux_t989_$TAG"
New-Item -ItemType Directory -Force $OUT | Out-Null
$oldDataDir = $env:PSMUX_DATA_DIR
$before = (& $PSMUX ls 2>$null | Out-String).Trim()

$bashExe = "C:\Program Files\Git\bin\bash.exe"
$haveBash = Test-Path $bashExe
$bash = $bashExe -replace '\\', '/'

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$feed = "$OUT\conptyfeed989.exe"
& $csc /nologo /out:$feed /platform:x64 "$TESTS\conptyfeed610.cs" 2>&1 | Out-Null
if (-not (Test-Path $feed)) { Write-Host "helper build failed" -ForegroundColor Red; exit 1 }

Write-Host "Binary: $PSMUX" -ForegroundColor Cyan

$script:ctx = @()

# Start a server in its own namespace with $cmd as the pane command, wait for
# $ready in the pane, then attach a real client under a pseudoconsole.
function New-Pane([string]$name, [string]$cmd, [string]$ready) {
    $sock = "t989${name}_$TAG"
    $dir = "$OUT\$name"
    New-Item -ItemType Directory -Force "$dir\data" | Out-Null
    $env:PSMUX_DATA_DIR = "$dir\data"
    & $PSMUX -L $sock new-session -d -s t -x 100 -y 30 $cmd 2>&1 | Out-Null
    $leaf = Split-Path $PSMUX -Leaf
    $srv = $null
    for ($k = 0; $k -lt 50 -and -not $srv; $k++) {
        $srv = Get-CimInstance Win32_Process -Filter "Name='$leaf'" |
            Where-Object { $_.CommandLine -match [regex]::Escape($sock) -and $_.CommandLine -match 'server' } |
            Select-Object -First 1
        if (-not $srv) { Start-Sleep -Milliseconds 100 }
    }
    $proc = $null
    if ($srv) { $proc = [Diagnostics.Process]::GetProcessById($srv.ProcessId); $null = $proc.Handle }
    $c = [pscustomobject]@{ Sock = $sock; Dir = $dir; Server = $proc; Client = $null; Ctrl = "$dir\c.ctrl"; Ready = $false }
    for ($i = 0; $i -lt 120; $i++) {
        Start-Sleep -Milliseconds 250
        if ((& $PSMUX -L $sock capture-pane -p -t t 2>&1 | Out-String) -match $ready) { $c.Ready = $true; break }
    }
    Set-Content $c.Ctrl "" -NoNewline
    $c.Client = Start-Process -FilePath $feed `
        -ArgumentList "`"$($c.Ctrl)`"", "`"$dir\c.bin`"", "`"$PSMUX`" -L $sock attach-session -t t" `
        -PassThru -WindowStyle Hidden
    $script:ctx += $c
    Start-Sleep -Seconds 2
    return $c
}

function Feed($c, [string]$line) { Add-Content $c.Ctrl $line }
function Cap($c) {
    $env:PSMUX_DATA_DIR = "$($c.Dir)\data"
    & $PSMUX -L $c.Sock capture-pane -p -t t 2>&1 | Out-String
}
function Alive($c) { $c.Server -and -not $c.Server.HasExited }
function ExitText($c) { if (Alive $c) { "alive" } else { '0x{0:X8}' -f $c.Server.ExitCode } }

# Run $cmd (with a unique marker in a trailing comment), press Ctrl+C after
# $settleMs, and require the shell's prompt to come back after the marked
# command line within 15 s: proof the command was interrupted.  The marker
# check does not type anything after the Ctrl+C, so it is not affected by
# conhost holding the last input record until the next key.
function Interrupt-Check($c, [string]$label, [string]$cmd, [int]$settleMs, [string]$prompt) {
    $m = "MK" + (Get-Random -Minimum 10000 -Maximum 99999)
    Feed $c ("LINE " + ($cmd -f $m))
    Start-Sleep -Milliseconds $settleMs
    $back = "(?s)$m.*\n\s*$prompt"
    $running = -not ((Cap $c) -match $back)
    Feed $c "HEX 03"
    $ok = $false
    for ($i = 0; $i -lt 30; $i++) {
        Start-Sleep -Milliseconds 500
        if ((Cap $c) -match $back) { $ok = $true; break }
    }
    if ($running -and $ok -and (Alive $c)) { Write-Pass "$label (server alive)" }
    else { Write-Fail "$label (was running=$running, prompt back=$ok, server=$(ExitText $c))" }
}

if ($haveBash) {
    Write-Host "`n=== Group 1: Git Bash with a native PROMPT_COMMAND, repeated Ctrl+C ===" -ForegroundColor Cyan
    $rc = "$OUT\bashrc_native.sh"
    "PROMPT_COMMAND='/c/Windows/System32/hostname.exe >/dev/null'`nPS1='READY$ '`n" | Set-Content $rc -Encoding ASCII -NoNewline
    $b = New-Pane "nat" "`"$bash`" --rcfile $($rc -replace '\\','/') -i" 'READY\$'
    if (-not $b.Ready) { Write-Fail "Git Bash prompt never appeared" }
    $gaps = @(30, 100, 250, -1)
    foreach ($g in $gaps) {
        $sent = 0
        for ($p = 0; $p -lt 25; $p++) {
            Feed $b "HEX 03"; $sent++
            $ms = if ($g -lt 0) { Get-Random -Minimum 20 -Maximum 400 } else { $g }
            Start-Sleep -Milliseconds $ms
            if (-not (Alive $b)) { break }
        }
        Start-Sleep -Seconds 2
        $gl = if ($g -lt 0) { "random 20..400" } else { "$g" }
        if (Alive $b) { Write-Pass "25 Ctrl+C presses, gap $gl ms: server alive" }
        else { Write-Fail "server died after $sent presses (gap $gl ms) with $(ExitText $b)"; break }
    }
    if (Alive $b) {
        $env:PSMUX_DATA_DIR = "$($b.Dir)\data"
        $has = & $PSMUX -L $b.Sock has-session -t t 2>&1; $hasOk = ($LASTEXITCODE -eq 0)
        if ($hasOk) { Write-Pass "session still exists" } else { Write-Fail "session is gone" }

        Write-Host "`n=== Group 2: Ctrl+C still interrupts programs in that Git Bash pane ===" -ForegroundColor Cyan
        Interrupt-Check $b "MSYS sleep 100 interrupted" "sleep 100 # {0}" 1500 'READY\$'
        Interrupt-Check $b "native ping -t interrupted" "/c/Windows/System32/PING.EXE -t 127.0.0.1 # {0}" 2500 'READY\$'
        Interrupt-Check $b "MSYS sleep 100 interrupted again" "sleep 100 # {0}" 1500 'READY\$'
    }

    Write-Host "`n=== Group 3: Git Bash with no PROMPT_COMMAND ===" -ForegroundColor Cyan
    $rc2 = "$OUT\bashrc_plain.sh"
    "PROMPT_COMMAND=`nPS1='READY$ '`n" | Set-Content $rc2 -Encoding ASCII -NoNewline
    $p2 = New-Pane "pln" "`"$bash`" --rcfile $($rc2 -replace '\\','/') -i" 'READY\$'
    for ($p = 0; $p -lt 25; $p++) { Feed $p2 "HEX 03"; Start-Sleep -Milliseconds 80 }
    Start-Sleep -Seconds 2
    if (Alive $p2) { Write-Pass "25 Ctrl+C presses: server alive" } else { Write-Fail "server died with $(ExitText $p2)" }
    if (Alive $p2) { Interrupt-Check $p2 "MSYS sleep 100 interrupted" "sleep 100 # {0}" 1500 'READY\$' }
} else {
    Write-Host "  [SKIP] Git Bash not installed at $bashExe" -ForegroundColor Yellow
}

Write-Host "`n=== Group 4: pwsh pane ===" -ForegroundColor Cyan
$pw = New-Pane "pws" "pwsh -NoLogo -NoProfile" 'PS [A-Z]:\\'
if (-not $pw.Ready) { Write-Fail "pwsh prompt never appeared" }
for ($p = 0; $p -lt 15; $p++) { Feed $pw "HEX 03"; Start-Sleep -Milliseconds 100 }
Start-Sleep -Seconds 1
if (Alive $pw) { Write-Pass "15 Ctrl+C presses at the pwsh prompt: server alive" } else { Write-Fail "server died with $(ExitText $pw)" }
Interrupt-Check $pw "Start-Sleep 100 interrupted" "Start-Sleep 100 # {0}" 2000 'PS [A-Z]:\\'
Interrupt-Check $pw "ping -t interrupted" "ping -t 127.0.0.1 # {0}" 2500 'PS [A-Z]:\\'

Write-Host "`n=== Group 5: cmd pane ===" -ForegroundColor Cyan
$cm = New-Pane "cmd" "cmd.exe /k prompt CMDREADY" 'CMDREADY'
if (-not $cm.Ready) { Write-Fail "cmd prompt never appeared" }
Interrupt-Check $cm "ping -t interrupted" "ping -t 127.0.0.1 & rem {0}" 2500 'CMDREADY'
for ($p = 0; $p -lt 15; $p++) { Feed $cm "HEX 03"; Start-Sleep -Milliseconds 100 }
Start-Sleep -Seconds 1
if (Alive $cm) { Write-Pass "15 Ctrl+C presses at the cmd prompt: server alive" } else { Write-Fail "server died with $(ExitText $cm)" }

# Clean up only our namespaces, by PID and by -L.
foreach ($c in $script:ctx) {
    if ($c.Client) { try { Stop-Process -Id $c.Client.Id -Force -EA Stop } catch {} }
    $env:PSMUX_DATA_DIR = "$($c.Dir)\data"
    & $PSMUX -L $c.Sock kill-server 2>&1 | Out-Null
}
$env:PSMUX_DATA_DIR = $oldDataDir
$after = (& $PSMUX ls 2>$null | Out-String).Trim()
if ($before -ne $after) { Write-Host "WARNING default namespace changed: [$before] -> [$after]" -ForegroundColor Yellow }

Write-Host "`nPassed: $script:TestsPassed  Failed: $script:TestsFailed"
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
