# Issue #744 / PR #745: a clipboard paste made while one of the client's own
# prompts is open must stay in that prompt and must not ALSO reach the pane.
#
# Windows Terminal binds Ctrl+V: the press never reaches the client, the text
# is injected as character key events, and only the V release (with Ctrl still
# held) is forwarded.  The characters go into the open prompt; the release
# fires the client's clipboard read back, which pushed `send-paste` to the
# pane unconditionally, so after Esc the text sat on the shell command line.
#
# This suite drives a REAL attached client at its console input buffer:
#   tests\injector.cs            prefix and prompt keys (C-b, :, ',', Esc, Enter)
#   tests\paste_host_injector.cs the paste itself, in two shapes:
#     host   characters, then the Ctrl+V release only (Windows Terminal)
#     release  the Ctrl+V release only, no press and no characters (the
#            host's binding pasted where the client cannot see it; the client
#            reads the clipboard itself).  A press AND a release with nothing
#            between them is not a paste since issue #858: that terminal does
#            not paste on Ctrl+V.
#
# The clipboard is real, so a desktop that refuses clipboard access to this
# process is detected up front and reported as SKIP, never as a pass.
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue744_paste_overlay_leak.ps1
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8
$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = "i744_$PID"
$S = "p744"
$script:Pass = 0; $script:Fail = 0; $script:Skip = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:Skip++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkGray }
function P { & $PSMUX -L $NS @args 2>&1 }

foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE') { Remove-Item "Env:\$v" -EA SilentlyContinue }
$savedDataDir = $env:PSMUX_DATA_DIR
$savedNoWarm = $env:PSMUX_NO_WARM
$root = Join-Path $env:TEMP "psmux_i744_$PID"
Remove-Item -Recurse -Force $root -EA SilentlyContinue
New-Item -ItemType Directory -Force $root | Out-Null
$env:PSMUX_DATA_DIR = Join-Path $root "data"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$env:PSMUX_NO_WARM = "1"

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$keys = Join-Path $root "injector.exe"
$paste = Join-Path $root "paste_host_injector.exe"
& $csc /nologo /optimize /out:$keys (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
& $csc /nologo /optimize /out:$paste (Join-Path $PSScriptRoot "paste_host_injector.cs") 2>&1 | Out-Null
if (-not (Test-Path $keys) -or -not (Test-Path $paste)) { Write-Host "FATAL: could not compile the injectors" -ForegroundColor Red; exit 1 }

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class I744Clip {
    [DllImport("user32.dll", SetLastError=true)] static extern bool OpenClipboard(IntPtr h);
    [DllImport("user32.dll")] static extern bool CloseClipboard();
    public static int Probe() {
        if (OpenClipboard(IntPtr.Zero)) { CloseClipboard(); return 0; }
        return Marshal.GetLastWin32Error();
    }
}
'@

Write-Host "`n=== Issue #744: a paste into a client prompt stays in the prompt ($PSMUX) ===" -ForegroundColor Cyan
Write-Info ("psmux: {0}" -f ((& $PSMUX -V) -join ' '))

$clipErr = [I744Clip]::Probe()
$clipOk = $false
if ($clipErr -eq 0) {
    $probe = "CLIPPROBE744_$PID"
    try { Set-Clipboard -Value $probe -EA Stop; $clipOk = ((Get-Clipboard -Raw) -eq $probe) } catch { $clipOk = $false }
}
if (-not $clipOk) {
    Write-Skip "UI-ACCESS-DENIED: the desktop refuses clipboard access to this process (OpenClipboard err=$clipErr); #744 is not measurable here"
    Write-Host ("`nResults: {0} passed, {1} failed, {2} skipped" -f $script:Pass, $script:Fail, $script:Skip)
    exit 0
}
$savedClip = try { Get-Clipboard -Raw -EA SilentlyContinue } catch { $null }

$client = $null
try {
    P kill-server | Out-Null
    P new-session -d -s $S -x 120 -y 30 | Out-Null
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 20000) {
        if (((P capture-pane -t $S -p) -join "`n") -match 'PS [A-Z]:\\') { break }
        Start-Sleep -Milliseconds 100
    }
    $client = Start-Process -FilePath $PSMUX -ArgumentList "-L", $NS, "attach-session", "-t", $S -PassThru -WindowStyle Normal
    $sw.Restart()
    while ($sw.ElapsedMilliseconds -lt 15000) {
        if (((P display-message -t $S -p '#{session_attached}') -join '').Trim() -eq '1') { break }
        Start-Sleep -Milliseconds 100
    }
    if (((P display-message -t $S -p '#{session_attached}') -join '').Trim() -ne '1') { Write-Fail "the attached client never registered"; throw "no client" }
    Write-Info "attached client pid=$($client.Id)"
    Start-Sleep -Seconds 2

    function Keys([string]$spec) { & $keys $client.Id $spec | Out-Null; return $LASTEXITCODE }
    function Count-OnPane([string]$text) {
        $cap = (P capture-pane -t $S -p) -join "`n"
        return ([regex]::Matches($cap, [regex]::Escape($text))).Count
    }
    function Pane-Tail { ((P capture-pane -t $S -p) | Where-Object { $_.Trim() -ne '' } | Select-Object -Last 2) -join ' | ' }
    function Reset-Pane {
        P send-keys -t $S Escape | Out-Null
        Start-Sleep -Milliseconds 250
        P send-keys -t $S "clear" Enter | Out-Null
        Start-Sleep -Milliseconds 900
    }
    function Do-Paste([string]$mode, [string]$text) {
        Set-Clipboard -Value $text
        Start-Sleep -Milliseconds 150
        & $paste $client.Id $mode 150 $text | Out-Null
        $rc = $LASTEXITCODE
        Start-Sleep -Milliseconds 1200
        return $rc
    }

    # ---- 1. command prompt, Windows Terminal shape, then Esc ---------------
    foreach ($mode in 'host', 'release') {
        Write-Host "`n[command prompt, $mode paste, Esc] the issue's exact steps" -ForegroundColor Yellow
        Reset-Pane
        $m = "LEAK744${mode}ESC"
        [void](Keys "^b{SLEEP:300}:{SLEEP:400}")
        $rc = Do-Paste $mode "display-message $m"
        if ($rc -eq 2) { Write-Skip "AttachConsole refused ($mode)"; continue }
        [void](Keys "{ESC}")
        Start-Sleep -Milliseconds 800
        $n = Count-OnPane $m
        if ($n -eq 0) { Write-Pass "$mode paste into the command prompt did not reach the pane" }
        else { Write-Fail "$mode paste into the command prompt LEAKED into the pane ($n time(s)): '$(Pane-Tail)'" }
    }

    # ---- 2. command prompt, paste then Enter: the prompt got it exactly once
    foreach ($mode in 'host', 'release') {
        Write-Host "`n[command prompt, $mode paste, Enter] the prompt holds the text once" -ForegroundColor Yellow
        Reset-Pane
        P set -u -g '@p744' | Out-Null
        $v = "VAL744$mode"
        [void](Keys "^b{SLEEP:300}:{SLEEP:400}")
        $rc = Do-Paste $mode "set -g @p744 $v"
        if ($rc -eq 2) { Write-Skip "AttachConsole refused ($mode)"; continue }
        [void](Keys "{ENTER}")
        Start-Sleep -Milliseconds 800
        $got = ((P show-options -gv '@p744') -join '').Trim()
        if ($got -eq $v) { Write-Pass "$mode paste: the prompt ran 'set -g @p744 $v' exactly once" }
        else { Write-Fail "$mode paste: @p744='$got', expected '$v' (prompt content duplicated or missing)" }
        $n = Count-OnPane $v
        if ($n -eq 0) { Write-Pass "$mode paste: nothing reached the pane" }
        else { Write-Fail "$mode paste: the text ALSO reached the pane ($n): '$(Pane-Tail)'" }
    }

    # ---- 3. rename-window prompt ------------------------------------------
    Write-Host "`n[rename-window prompt, host paste, Enter]" -ForegroundColor Yellow
    Reset-Pane
    $orig = ((P display-message -t $S -p '#{window_name}') -join '').Trim()
    [void](Keys "^b{SLEEP:300},{SLEEP:400}")
    $rc = Do-Paste 'host' 'WIN744'
    if ($rc -ne 2) {
        [void](Keys "{ENTER}")
        Start-Sleep -Milliseconds 800
        $wn = ((P display-message -t $S -p '#{window_name}') -join '').Trim()
        # The prompt may open pre filled with the current name, so judge the
        # paste by how many times it appears in the new name: exactly once.
        $k = ([regex]::Matches($wn, 'WIN744')).Count
        if ($k -eq 1) { Write-Pass "the rename prompt took the paste once: window_name=$wn" }
        else { Write-Fail "window_name='$wn' holds WIN744 $k time(s), expected 1" }
        $n = Count-OnPane 'WIN744'
        if ($n -eq 0) { Write-Pass "the rename paste did not reach the pane" }
        else { Write-Fail "the rename paste LEAKED into the pane: '$(Pane-Tail)'" }
    } else { Write-Skip "AttachConsole refused (rename)" }

    # ---- 3b. window index prompt (prefix '), keeps digits only -----------
    Write-Host "`n[window index prompt, host paste 'QX9QY', Esc]" -ForegroundColor Yellow
    Reset-Pane
    [void](Keys "^b{SLEEP:300}'{SLEEP:400}")
    $rc = Do-Paste 'host' 'QX9QY'
    if ($rc -ne 2) {
        [void](Keys "{ESC}")
        Start-Sleep -Milliseconds 800
        $cap = (P capture-pane -t $S -p) -join "`n"
        if ($cap -notmatch 'QX|QY|X9Q') { Write-Pass "nothing of the paste reached the pane from the window index prompt" }
        else { Write-Fail "the window index prompt paste LEAKED into the pane: '$(Pane-Tail)'" }
    } else { Write-Skip "AttachConsole refused (window index)" }

    # tmux binds ' as a plain `command-prompt -pindex { select-window -t ':%%' }`
    # (key-bindings.c:394): letters belong to the prompt, never to the pane,
    # and a window NAME selects that window.
    Write-Host "`n[window index prompt, letters typed by hand, then a name]" -ForegroundColor Yellow
    Reset-Pane
    [void](Keys "^b{SLEEP:300}'{SLEEP:400}q{SLEEP:150}w{SLEEP:150}{ESC}")
    Start-Sleep -Milliseconds 800
    $tail = ((P capture-pane -t $S -p) | Where-Object { $_.Trim() } | Select-Object -Last 1)
    if ($tail -notmatch '>\s*qw\s*$') { Write-Pass "letters typed at the window index prompt stayed out of the pane: '$($tail.Trim())'" }
    else { Write-Fail "letters typed at the window index prompt reached the pane: '$($tail.Trim())'" }
    P new-window -d -t $S -n idxname744 | Out-Null
    Start-Sleep -Milliseconds 500
    [void](Keys "^b{SLEEP:300}'{SLEEP:400}idxname744{SLEEP:200}{ENTER}")
    Start-Sleep -Milliseconds 800
    $wn = ((P display-message -t $S -p '#{window_name}') -join '').Trim()
    if ($wn -eq 'idxname744') { Write-Pass "a window name typed at the index prompt selects it, as tmux's select-window -t ':%%' does" }
    else { Write-Fail "typing 'idxname744' at the index prompt left window '$wn' active" }
    P select-window -t "${S}:0" | Out-Null
    P kill-window -t "${S}:idxname744" | Out-Null
    Start-Sleep -Milliseconds 400

    # ---- 3c. copy mode search prompt (vi '/') ------------------------------
    Write-Host "`n[copy mode search prompt, host paste, Esc, q]" -ForegroundColor Yellow
    Reset-Pane
    P set -g mode-keys vi | Out-Null
    P copy-mode -t $S | Out-Null
    Start-Sleep -Milliseconds 400
    [void](Keys "/{SLEEP:400}")
    $rc = Do-Paste 'host' 'SRCH744'
    if ($rc -ne 2) {
        [void](Keys "{ESC}{SLEEP:300}q")
        Start-Sleep -Milliseconds 800
        $inMode = ((P display-message -t $S -p '#{pane_in_mode}') -join '').Trim()
        if ($inMode -eq '1') { [void](Keys "q"); Start-Sleep -Milliseconds 500 }
        $n = Count-OnPane 'SRCH744'
        if ($n -eq 0) { Write-Pass "the copy mode search paste did not reach the shell line" }
        else { Write-Fail "the copy mode search paste reached the pane ($n): '$(Pane-Tail)'" }
    } else { Write-Skip "AttachConsole refused (copy search)" }
    P set -g mode-keys emacs | Out-Null

    # ---- 3d. a large paste into the command prompt ------------------------
    Write-Host "`n[command prompt, 4000 character host paste, Esc]" -ForegroundColor Yellow
    Reset-Pane
    [void](Keys "^b{SLEEP:300}:{SLEEP:400}")
    $rc = Do-Paste 'host' ("BIG744" + ("k" * 4000))
    if ($rc -ne 2) {
        Start-Sleep -Milliseconds 800
        [void](Keys "{ESC}")
        Start-Sleep -Milliseconds 800
        $n = Count-OnPane 'BIG744'
        $kk = ((P capture-pane -t $S -p) -join '') -match 'kkkkkkkkkk'
        if ($n -eq 0 -and -not $kk) { Write-Pass "a 4000 character paste into the prompt did not reach the pane" }
        else { Write-Fail "the large paste reached the pane (BIG744 x$n, run of k: $kk)" }
    } else { Write-Skip "AttachConsole refused (large)" }

    # ---- 4. no overlay: the common case still lands once in the pane -----
    foreach ($mode in 'host', 'release') {
        Write-Host "`n[no prompt open, $mode paste] the pane gets it exactly once" -ForegroundColor Yellow
        Reset-Pane
        $t = "PANE744$mode"
        $rc = Do-Paste $mode $t
        if ($rc -eq 2) { Write-Skip "AttachConsole refused ($mode)"; continue }
        $n = Count-OnPane $t
        if ($n -eq 1) { Write-Pass "$mode paste with no prompt open landed in the pane exactly once" }
        else { Write-Fail "$mode paste with no prompt open landed $n time(s): '$(Pane-Tail)'" }
    }

    # ---- 5. a paste with spaces, repeated, into the prompt ----------------
    Write-Host "`n[command prompt, ' abc def' pasted 5 times quickly] no space reaches the pane" -ForegroundColor Yellow
    Reset-Pane
    P send-keys -t $S "ZZ744" | Out-Null
    Start-Sleep -Milliseconds 500
    [void](Keys "^b{SLEEP:300}:{SLEEP:400}")
    for ($i = 0; $i -lt 5; $i++) {
        Set-Clipboard -Value " abc def"
        & $paste $client.Id 'host' 60 " abc def" | Out-Null
        Start-Sleep -Milliseconds 250
    }
    Start-Sleep -Milliseconds 600
    [void](Keys "{ESC}")
    Start-Sleep -Milliseconds 800
    $line = ((P capture-pane -t $S -p) | Where-Object { $_ -match 'ZZ744' } | Select-Object -Last 1)
    if ($line -match 'ZZ744\s*$') { Write-Pass "the shell line is still just 'ZZ744': '$($line.Trim())'" }
    else { Write-Fail "the shell line changed behind the prompt: '$line'" }

    Write-Host "`n[TUI] the attached client is still healthy" -ForegroundColor Yellow
    $att = ((P display-message -t $S -p '#{session_attached}') -join '').Trim()
    if ($att -eq '1') { Write-Pass "the client is still attached" } else { Write-Fail "session_attached=$att" }
}
catch { if ($_.Exception.Message -ne 'no client') { Write-Fail "unexpected: $_" } }
finally {
    if ($client) { Stop-Process -Id $client.Id -Force -EA SilentlyContinue }
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 300
    $env:PSMUX_DATA_DIR = $savedDataDir
    $env:PSMUX_NO_WARM = $savedNoWarm
    if ($null -ne $savedClip -and $savedClip -ne '') { try { Set-Clipboard -Value $savedClip } catch {} }
    Remove-Item -Recurse -Force $root -EA SilentlyContinue
}

Write-Host ("`nResults: {0} passed, {1} failed, {2} skipped" -f $script:Pass, $script:Fail, $script:Skip)
exit $script:Fail
