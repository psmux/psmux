# Issue #828: send-keys -t <window> -l <text> was captured by a mode or an
# overlay that did not belong to the target pane (dracic, 3.4.0).
#
# With no client attached:
#   * copy mode or clock mode opened on window 0 swallowed a send aimed at
#     window 1 (tmux 3.4 types the text into window 1 untouched)
#   * #{pane_in_mode} read 1 on every window while one pane was in a mode
#     (tmux: only the moded pane, format.c format_cb_pane_in_mode)
#   * an open display-popup received the text, a display-menu ate the Enter
#     and a confirm-before prompt was ANSWERED by the leading Y of the text
#     and ran its command
#   * every swallowed send exited 0
#
# tmux: cmd-send-keys.c hands each key to the TARGET pane through
# window_pane_key (window.c), which gives it to that pane's own mode if the
# pane is in one, else to the pane's program.  Popups, menus and prompts are
# client overlays that only the client's own keys reach.  Copy mode on the
# target pane itself still takes the text, in tmux too; that is kept.
#
# Kept psmux behaviour: a send-keys with no target, or naming only the
# session, still drives an open overlay (scripts close popups with
# `send-keys -t <session> Escape`).
#
# A reader program in window 1 logs every stdin chunk as hex, so the suite
# sees exactly what the pane received.  Set PSMUX_TEST_BIN to test a
# non-installed binary.  Exits with the failure count.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:Pass = 0; $script:Fail = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Check([bool]$ok, [string]$m) { if ($ok) { Write-Pass $m } else { Write-Fail $m } }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','SHELL') {
    Remove-Item "Env:$v" -ErrorAction SilentlyContinue
}
$env:PSMUX_NO_WARM = '1'
$NS = "t828" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$rig = Join-Path $env:TEMP "psmux828-$NS"
New-Item -ItemType Directory -Force -Path $rig | Out-Null
$conf = Join-Path $rig 'empty.conf'; Set-Content $conf ''
$env:PSMUX_CONFIG_FILE = $conf
$reader = Join-Path $rig 'reader.ps1'
Set-Content -Path $reader -Value @'
param([string]$Log)
Add-Type -Namespace R -Name K -MemberDefinition @"
[DllImport("kernel32.dll")] public static extern System.IntPtr GetStdHandle(int n);
[DllImport("kernel32.dll")] public static extern bool GetConsoleMode(System.IntPtr h, out uint m);
[DllImport("kernel32.dll")] public static extern bool SetConsoleMode(System.IntPtr h, uint m);
"@
$h = [R.K]::GetStdHandle(-10); $m = 0; [void][R.K]::GetConsoleMode($h, [ref]$m)
[void][R.K]::SetConsoleMode($h, (($m -band (-bnot 0x47)) -bor 0x200 -bor 0x80))
[Console]::InputEncoding = [Text.Encoding]::UTF8
$w = [IO.StreamWriter]::new($Log, $true); $w.AutoFlush = $true; $w.WriteLine('START')
$in = [Console]::OpenStandardInput(); $buf = [byte[]]::new(65536)
while ($true) { $n = $in.Read($buf, 0, $buf.Length); if ($n -gt 0) { $w.WriteLine('chunk ' + [BitConverter]::ToString($buf, 0, $n).Replace('-', '').ToLower()) } }
'@

function P { & $PSMUX -L $NS @args }
$Text = 'Yes, continue with the next step.'
$TextHex = ([BitConverter]::ToString([Text.Encoding]::UTF8.GetBytes($Text))).Replace('-', '').ToLower()
$S = 'r828'

function Wait-Start([string]$log) {
    for ($i = 0; $i -lt 150; $i++) {
        if ((Test-Path $log) -and (Select-String -Quiet -Path $log -Pattern START)) { return $true }
        Start-Sleep -Milliseconds 100
    }
    return $false
}
function Received([string]$log) {
    ((Get-Content $log -EA SilentlyContinue | Where-Object { $_ -like 'chunk *' }) | ForEach-Object { $_.Substring(6) }) -join ''
}
function Mode-Of([string]$t) { "$(P display-message -p -t $t '#{pane_in_mode}')".Trim() }
function ModeName-Of([string]$t) { "$(P display-message -p -t $t '#{pane_mode}')".Trim() }

# Fresh server: window 0 a plain shell, window 1 the reader.  Returns the log.
function New-Rig {
    P kill-server 2>&1 | Out-Null; Start-Sleep -Milliseconds 300
    $log = Join-Path $rig ("r-" + [guid]::NewGuid().ToString('N').Substring(0, 6) + '.log')
    P new-session -d -s $S -x 120 -y 40
    P new-window -t "${S}:" -n reader pwsh -NoProfile -File $reader $log
    if (-not (Wait-Start $log)) { Write-Fail "reader did not start" }
    Start-Sleep -Milliseconds 800
    return $log
}
function Send-Text {
    P send-keys -t "${S}:1" -l $Text; $script:rc1 = $LASTEXITCODE
    Start-Sleep -Milliseconds 300
    P send-keys -t "${S}:1" Enter; $script:rc2 = $LASTEXITCODE
    Start-Sleep -Milliseconds 1200
}

try {
    Write-Host "`n[1] control: nothing open" -ForegroundColor Yellow
    $log = New-Rig
    Send-Text
    Check ((Received $log) -eq "${TextHex}0d") "text and CR arrive (got '$(Received $log)')"

    Write-Host "`n[2] copy mode on ANOTHER window" -ForegroundColor Yellow
    $log = New-Rig
    P copy-mode -t "${S}:0"; Start-Sleep -Milliseconds 500
    Check ((Mode-Of "${S}:0") -eq '1') "pane_in_mode of the moded pane is 1"
    Check ((Mode-Of "${S}:1") -eq '0') "pane_in_mode of the other window is 0"
    Send-Text
    Check ((Received $log) -eq "${TextHex}0d") "text and CR reach window 1 (got '$(Received $log)')"
    Check ($rc1 -eq 0 -and $rc2 -eq 0) "send-keys exits 0"
    Check ((ModeName-Of "${S}:0") -eq 'copy-mode') "window 0 is still in copy mode afterwards"
    P send-keys -t "${S}:0" -X cancel; Start-Sleep -Milliseconds 400
    Check ((Mode-Of "${S}:0") -eq '0') "send-keys -X cancel on window 0 leaves its copy mode"
    Check ((Mode-Of "${S}:1") -eq '0') "window 1 still not in a mode"

    Write-Host "`n[3] clock mode on ANOTHER window" -ForegroundColor Yellow
    $log = New-Rig
    P clock-mode -t "${S}:0"; Start-Sleep -Milliseconds 500
    Check ((ModeName-Of "${S}:0") -eq 'clock-mode') "pane_mode of window 0 is clock-mode"
    Check ((Mode-Of "${S}:1") -eq '0') "pane_in_mode of window 1 is 0"
    Send-Text
    Check ((Received $log) -eq "${TextHex}0d") "text and CR reach window 1 (got '$(Received $log)')"
    Check ((ModeName-Of "${S}:0") -eq 'clock-mode') "window 0 keeps its clock"
    P send-keys -t "${S}:0" x; Start-Sleep -Milliseconds 400
    Check ((Mode-Of "${S}:0") -eq '0') "a key sent to the clock pane ends its clock mode (window_clock_key)"

    Write-Host "`n[4] copy mode on the TARGET pane (tmux swallows too: kept)" -ForegroundColor Yellow
    $log = New-Rig
    P copy-mode -t "${S}:1"; Start-Sleep -Milliseconds 500
    Check ((Mode-Of "${S}:1") -eq '1') "pane_in_mode of window 1 is 1"
    Check ((Mode-Of "${S}:0") -eq '0') "pane_in_mode of window 0 is 0"
    P send-keys -t "${S}:1" -l 'abc'; Start-Sleep -Milliseconds 800
    Check ((Received $log) -eq '') "copy mode on the target takes the text (got '$(Received $log)')"
    Check ((Mode-Of "${S}:1") -eq '1') "window 1 still in copy mode"

    Write-Host "`n[5] copy cursor of the focused pane survives targeted commands" -ForegroundColor Yellow
    $log = New-Rig
    P select-window -t "${S}:0"
    P copy-mode -t "${S}:0"; Start-Sleep -Milliseconds 300
    P send-keys -t "${S}:0" -X cursor-up; P send-keys -t "${S}:0" -X cursor-up; P send-keys -t "${S}:0" -X cursor-right
    Start-Sleep -Milliseconds 300
    $before = "$(P display-message -p -t "${S}:0" '#{copy_cursor_x},#{copy_cursor_y}')".Trim()
    P display-message -p -t "${S}:1" '#{pane_in_mode}' | Out-Null
    Send-Text
    $after = "$(P display-message -p -t "${S}:0" '#{copy_cursor_x},#{copy_cursor_y}')".Trim()
    Check ($before -eq $after -and $before -ne '') "copy cursor unchanged ($before -> $after)"
    Check ((Received $log) -eq "${TextHex}0d") "text reaches window 1 while window 0 (focused) is in copy mode"
    $mode0 = "$(P display-message -p '#{pane_in_mode}')".Trim()
    Check ($mode0 -eq '1') "the focused pane is still in copy mode (untargeted query: $mode0)"

    Write-Host "`n[6] confirm-before is not answered by a window-targeted send" -ForegroundColor Yellow
    $log = New-Rig
    P confirm-before -t "${S}:0" -p 'really?' set -g status-left CONFIRM-RAN; Start-Sleep -Milliseconds 500
    Send-Text
    Check ((Received $log) -eq "${TextHex}0d") "text and CR reach window 1 (got '$(Received $log)')"
    $sl = "$(P show-options -g -v status-left)".Trim()
    Check ($sl -ne 'CONFIRM-RAN') "the confirm command did not run (status-left '$sl')"
    # Compatibility: a session-only send-keys still answers the prompt.
    P send-keys -t $S y; Start-Sleep -Milliseconds 500
    $sl = "$(P show-options -g -v status-left)".Trim()
    Check ($sl -eq 'CONFIRM-RAN') "the prompt stayed open; a session-only send-keys y answers it"

    Write-Host "`n[7] display-menu does not eat a window-targeted Enter" -ForegroundColor Yellow
    $log = New-Rig
    P display-menu -t "${S}:0" -x 0 -y 0 'Set status' s 'set -g status-left MENU-RAN'; Start-Sleep -Milliseconds 500
    Send-Text
    Check ((Received $log) -eq "${TextHex}0d") "text and CR reach window 1 (got '$(Received $log)')"
    $sl = "$(P show-options -g -v status-left)".Trim()
    Check ($sl -ne 'MENU-RAN') "no menu item ran (status-left '$sl')"

    Write-Host "`n[8] display-popup does not take a window-targeted send" -ForegroundColor Yellow
    $log = New-Rig
    $plog = Join-Path $rig 'popup.log'; Set-Content $plog ''
    P display-popup -t "${S}:0" -E pwsh -NoProfile -File $reader $plog
    if (-not (Wait-Start $plog)) { Write-Fail "popup reader did not start" }
    Start-Sleep -Milliseconds 800
    Send-Text
    Check ((Received $log) -eq "${TextHex}0d") "text and CR reach window 1 (got '$(Received $log)')"
    Check ((Received $plog) -eq '') "the popup program received nothing (got '$(Received $plog)')"
    # Compatibility: a session-only send-keys still types into the popup.
    P send-keys -t $S -l 'zz'; Start-Sleep -Milliseconds 800
    Check ((Received $plog) -eq '7a7a') "a session-only send-keys still reaches the popup (got '$(Received $plog)')"

    # -----------------------------------------------------------------------
    # [9] An attached client: its own keystrokes (WriteConsoleInput) still
    # drive its copy mode and its confirm prompt, while targeted sends from a
    # script reach window 1 untouched.
    # -----------------------------------------------------------------------
    Write-Host "`n[9] attached client keeps its copy mode and prompt" -ForegroundColor Yellow
    P kill-server 2>&1 | Out-Null; Start-Sleep -Milliseconds 300
    $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
    $keyInj = Join-Path $rig "keys.exe"
    if (Test-Path $csc) { & $csc /nologo /optimize /out:$keyInj (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null }
    if (-not (Test-Path $keyInj)) {
        Write-Host "  [SKIP] csc.exe unavailable" -ForegroundColor DarkYellow
    } else {
        $launch = Join-Path $rig "launch.cmd"
@"
@echo off
set PSMUX_SESSION=
set PSMUX_SESSION_NAME=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX_NO_WARM=1
"$PSMUX" -L $NS new-session -s $S -x 120 -y 30
"@ | Set-Content -Path $launch -Encoding ASCII
        $null = Start-Process -FilePath $launch -PassThru
        $cli = $null
        for ($i = 0; $i -lt 60 -and -not $cli; $i++) {
            Start-Sleep -Milliseconds 250
            $cli = Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" |
                Where-Object { $_.CommandLine -match "-L $NS new-session -s $S" } | Select-Object -First 1
        }
        Start-Sleep -Seconds 3
        if (-not $cli) {
            Write-Host "  [SKIP] attached client did not start" -ForegroundColor DarkYellow
        } else {
            $cpid = [int]$cli.ProcessId
            try {
                $log = Join-Path $rig 'attached.log'; Set-Content $log ''
                P new-window -d -t "${S}:" -n reader pwsh -NoProfile -File $reader $log
                if (-not (Wait-Start $log)) { Write-Fail "reader did not start" }
                Start-Sleep -Milliseconds 800
                & $keyInj $cpid "^b{SLEEP:400}[" 2>&1 | Out-Null
                Start-Sleep -Milliseconds 1200
                if ((Mode-Of "${S}:0") -ne '1') {
                    Write-Host "  [SKIP] prefix-[ did not enter copy mode (injection refused)" -ForegroundColor DarkYellow
                } else {
                    Check ((Mode-Of "${S}:1") -eq '0') "client copy mode is on window 0 only"
                    Send-Text
                    Check ((Received $log) -eq "${TextHex}0d") "a script's send to window 1 arrives while the client is in copy mode (got '$(Received $log)')"
                    Check ((Mode-Of "${S}:0") -eq '1') "the client's pane is still in copy mode"
                    & $keyInj $cpid "q" 2>&1 | Out-Null
                    Start-Sleep -Milliseconds 1000
                    Check ((Mode-Of "${S}:0") -eq '0') "the client's own q still leaves copy mode"

                    P confirm-before -p 'really?' set -g status-left KEYS-RAN; Start-Sleep -Milliseconds 700
                    # The reader holds its log open: compare what is appended.
                    $base = Received $log
                    Send-Text
                    $more = (Received $log).Substring($base.Length)
                    Check ($more -eq "${TextHex}0d") "a script's send to window 1 passes the client's prompt (got '$more')"
                    Check ("$(P show-options -g -v status-left)".Trim() -ne 'KEYS-RAN') "the script's text did not answer the prompt"
                    & $keyInj $cpid "y" 2>&1 | Out-Null
                    Start-Sleep -Milliseconds 1000
                    Check ("$(P show-options -g -v status-left)".Trim() -eq 'KEYS-RAN') "the client's own y answers its prompt"
                }
            } finally {
                try { Stop-Process -Id $cpid -Force -EA SilentlyContinue } catch {}
                $cmdHost = Get-CimInstance Win32_Process -EA SilentlyContinue |
                    Where-Object { $_.CommandLine -and $_.CommandLine.Contains($launch) }
                foreach ($c in $cmdHost) { try { Stop-Process -Id $c.ProcessId -Force -EA SilentlyContinue } catch {} }
            }
        }
    }
}
finally {
    P kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
    Remove-Item -Recurse -Force $rig -EA SilentlyContinue
}

Write-Host "`nResult: $($script:Pass) passed, $($script:Fail) failed" -ForegroundColor Cyan
exit $script:Fail
