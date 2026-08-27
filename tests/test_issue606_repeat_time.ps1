# Issue #606: `set-option -g repeat-time N` in a config file was rejected with
# "unknown option 'repeat-time'".
#
# repeat-time lived in docs/configuration.md, in the option catalog and in the
# server's set-option, but src/config.rs `parse_option_value` had no arm for
# it, so a `.tmux.conf` line fell through to the hyphenated catch-all: the
# value went into `user_options`, the parser warned, and `repeat_time_ms` kept
# its 500 ms default. `set-option -g repeat-time 0` therefore silently failed
# to disable `bind-key -r` repeat, which is exactly what the reporter wanted.
#
# The fix routes repeat-time through the real field on the config path, bounds
# it like tmux (options-table.c: 0..=2000000 ms, "value is too small" /
# "value is too large"), lists it in the `show-options -g` dump and restores
# the 500 ms default on `-u`.
#
# Layers: CLI set/show, raw TCP verbs, config-file boot (config-warnings.log
# asserted clean), source-file, and a Win32 TUI section that injects real
# keystrokes into an attached client to prove the repeat window itself honours
# the value.
#
# Set PSMUX_TEST_BIN to test a non-installed binary.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$dataDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$TMP = Join-Path $env:TEMP "psmux_606"
New-Item -ItemType Directory -Force -Path $TMP | Out-Null
$SOCK = "i606"
$script:Pass = 0; $script:Fail = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

Write-Host "binary:  $PSMUX" -ForegroundColor Cyan
Write-Host "dataDir: $dataDir" -ForegroundColor Cyan

$emptyConf = Join-Path $TMP "empty.conf"
"" | Set-Content -Path $emptyConf -Encoding ASCII

function Kill-Sess([string]$n) {
    & $PSMUX -L $SOCK kill-session -t $n 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$dataDir\${SOCK}__$n.*" -Force -EA SilentlyContinue
}
function Show1([string]$sess) { (& $PSMUX -L $SOCK show-options -g repeat-time 2>&1 | Out-String).Trim() }
function ShowV() { (& $PSMUX -L $SOCK show-options -gv repeat-time 2>&1 | Out-String).Trim() }

# ---------------------------------------------------------------------------
# Part A: CLI set-option / show-options
# ---------------------------------------------------------------------------
Write-Host "`n=== Part A: CLI set-option / show-options ===" -ForegroundColor Cyan
$SA = "t606a"
Kill-Sess $SA
& $PSMUX -L $SOCK -f $emptyConf new-session -d -s $SA -x 100 -y 30 2>&1 | Out-Null
Start-Sleep -Seconds 3

$def = Show1 $SA
if ($def -eq "repeat-time 500") { Write-Pass "default is '$def' (tmux prints 'repeat-time 500')" }
else { Write-Fail "default is '$def', expected 'repeat-time 500'" }

foreach ($v in @("0", "250", "3000", "2000000")) {
    $out = (& $PSMUX -L $SOCK set-option -g repeat-time $v 2>&1 | Out-String).Trim()
    $rc = $LASTEXITCODE
    $got = Show1 $SA
    $gotv = ShowV
    if ($rc -eq 0 -and $out -eq "" -and $got -eq "repeat-time $v" -and $gotv -eq $v) {
        Write-Pass "set -g repeat-time $v -> show '$got', -gv '$gotv'"
    } else { Write-Fail "set -g repeat-time $v -> rc=$rc out='$out' show='$got' -gv='$gotv'" }
}

# tmux: `show-options -g` lists repeat-time. psmux answered the single-option
# query but omitted it from the full dump (same shape as #559).
& $PSMUX -L $SOCK set-option -g repeat-time 1234 2>&1 | Out-Null
$dump = (& $PSMUX -L $SOCK show-options -g 2>&1 | Out-String) -split "`r?`n" |
    Where-Object { $_ -match '^\s*repeat-time\b' }
if ($dump -and ($dump -join ' ') -match 'repeat-time\s+"?1234"?') {
    Write-Pass "show-options -g lists it: '$(($dump -join '; ').Trim())'"
} else { Write-Fail "show-options -g does not list repeat-time (got '$($dump -join '; ')')" }

# Validation, tmux parity (options-table.c: number, minimum 0, maximum 2000000).
& $PSMUX -L $SOCK set-option -g repeat-time 700 2>&1 | Out-Null
foreach ($bad in @("abc", "2000001", "-5", "99999999999")) {
    $out = (& $PSMUX -L $SOCK set-option -g repeat-time $bad 2>&1 | Out-String).Trim()
    $rc = $LASTEXITCODE
    $after = Show1 $SA
    if ($rc -ne 0 -and $after -eq "repeat-time 700") {
        Write-Pass "rejected '$bad' (rc=$rc, '$out'), value still 700"
    } else { Write-Fail "'$bad' -> rc=$rc out='$out' value now '$after' (expected rejection, value 700)" }
}

# -u restores the tmux table default.
& $PSMUX -L $SOCK set-option -gu repeat-time 2>&1 | Out-Null
Start-Sleep -Milliseconds 400
$afterUnset = Show1 $SA
if ($afterUnset -eq "repeat-time 500") { Write-Pass "set -gu restores the 500 ms default" }
else { Write-Fail "set -gu left '$afterUnset', expected 'repeat-time 500'" }

# ---------------------------------------------------------------------------
# Part B: raw TCP control verb (no CLI guard in front of it)
# ---------------------------------------------------------------------------
Write-Host "`n=== Part B: raw TCP set-option ===" -ForegroundColor Cyan
$portFile = "$dataDir\${SOCK}__$SA.port"
if (-not (Test-Path $portFile)) { $portFile = "$dataDir\$SA.port" }
if (Test-Path $portFile) {
    $port = (Get-Content $portFile -Raw).Trim()
    $keyFile = $portFile -replace '\.port$', '.key'
    $authKey = (Get-Content $keyFile -Raw).Trim()
    $tcp = New-Object System.Net.Sockets.TcpClient
    $tcp.Connect("127.0.0.1", [int]$port); $tcp.NoDelay = $true; $tcp.ReceiveTimeout = 2000
    $stream = $tcp.GetStream()
    $wr = New-Object System.IO.StreamWriter($stream); $wr.AutoFlush = $true; $wr.NewLine = "`n"
    $rd = New-Object System.IO.StreamReader($stream)
    $wr.WriteLine("AUTH $authKey"); $null = $rd.ReadLine()
    $wr.WriteLine("PERSISTENT")
    $wr.Write("set-option -g repeat-time 1750`n")
    Start-Sleep -Milliseconds 800
    $viaTcp = Show1 $SA
    if ($viaTcp -eq "repeat-time 1750") { Write-Pass "TCP set-option applied: '$viaTcp'" }
    else { Write-Fail "TCP set-option -> '$viaTcp', expected 'repeat-time 1750'" }
    # The server must refuse an out of range value on this route too, where no
    # CLI guard ever ran.
    $wr.Write("set-option -g repeat-time 5000000`n")
    Start-Sleep -Milliseconds 800
    $viaTcpBad = Show1 $SA
    if ($viaTcpBad -eq "repeat-time 1750") { Write-Pass "TCP out-of-range value refused, still '$viaTcpBad'" }
    else { Write-Fail "TCP accepted an out-of-range value: '$viaTcpBad'" }
    $tcp.Close()
} else { Write-Fail "no port file for $SA at $portFile" }
Kill-Sess $SA

# ---------------------------------------------------------------------------
# Part C: config file at server boot (the reporter's route)
# ---------------------------------------------------------------------------
Write-Host "`n=== Part C: config file boot ===" -ForegroundColor Cyan
$SC = "t606c"
$conf = Join-Path $TMP "repeat.conf"
"set-option -g repeat-time 250" | Set-Content -Path $conf -Encoding ASCII
Kill-Sess $SC
Remove-Item "$dataDir\config-warnings.log" -Force -EA SilentlyContinue
$env:PSMUX_CONFIG_FILE = $conf
$bootOut = (& $PSMUX -L $SOCK new-session -d -s $SC -x 100 -y 30 2>&1 | Out-String)
Remove-Item Env:PSMUX_CONFIG_FILE -EA SilentlyContinue
Start-Sleep -Seconds 3

$warnText = ""
if (Test-Path "$dataDir\config-warnings.log") { $warnText = (Get-Content "$dataDir\config-warnings.log" -Raw) }
Write-Info "boot stderr: $(($bootOut -split "`r?`n" | Where-Object { $_ -match '\S' }) -join ' | ')"
Write-Info "config-warnings.log: $(($warnText -split "`r?`n" | Where-Object { $_ -match '\S' }) -join ' | ')"
if ($bootOut -notmatch "unknown option 'repeat-time'") { Write-Pass "boot stderr has no unknown-option warning" }
else { Write-Fail "boot stderr still reports unknown option 'repeat-time'" }
if ($warnText -notmatch "repeat-time") { Write-Pass "config-warnings.log is clean of repeat-time" }
else { Write-Fail "config-warnings.log still mentions repeat-time" }
$fromConf = Show1 $SC
if ($fromConf -eq "repeat-time 250") { Write-Pass "config value reached the option: '$fromConf'" }
else { Write-Fail "config value ignored: show says '$fromConf', expected 'repeat-time 250'" }

# An out of range config value must be reported, not silently stored.
$SC2 = "t606c2"
$badConf = Join-Path $TMP "repeat_bad.conf"
"set-option -g repeat-time 9000000" | Set-Content -Path $badConf -Encoding ASCII
Kill-Sess $SC2
Remove-Item "$dataDir\config-warnings.log" -Force -EA SilentlyContinue
$env:PSMUX_CONFIG_FILE = $badConf
$bootOut2 = (& $PSMUX -L $SOCK new-session -d -s $SC2 -x 100 -y 30 2>&1 | Out-String)
Remove-Item Env:PSMUX_CONFIG_FILE -EA SilentlyContinue
Start-Sleep -Seconds 3
$warn2 = ""
if (Test-Path "$dataDir\config-warnings.log") { $warn2 = (Get-Content "$dataDir\config-warnings.log" -Raw) }
$val2 = Show1 $SC2
if (($bootOut2 + $warn2) -match 'value is too large: 9000000') {
    Write-Pass "out-of-range config value warned like tmux ('value is too large: 9000000')"
} else { Write-Fail "no 'value is too large' warning for 9000000 (stderr+log: '$(($bootOut2 + $warn2) -replace "`r?`n", ' | ')')" }
if ($val2 -eq "repeat-time 500") { Write-Pass "out-of-range config value left the default in place" }
else { Write-Fail "out-of-range config value was stored: '$val2'" }
Kill-Sess $SC2

# ---------------------------------------------------------------------------
# Part D: source-file at runtime
# ---------------------------------------------------------------------------
Write-Host "`n=== Part D: source-file ===" -ForegroundColor Cyan
$srcConf = Join-Path $TMP "repeat_src.conf"
"set -g repeat-time 1321" | Set-Content -Path $srcConf -Encoding ASCII
& $PSMUX -L $SOCK source-file $srcConf 2>&1 | Out-Null
Start-Sleep -Milliseconds 800
$afterSrc = Show1 $SC
if ($afterSrc -eq "repeat-time 1321") { Write-Pass "source-file applied: '$afterSrc'" }
else { Write-Fail "source-file did not apply: '$afterSrc', expected 'repeat-time 1321'" }
Kill-Sess $SC

# ---------------------------------------------------------------------------
# Part E: Win32 TUI. Real keystrokes into an attached client prove the repeat
# window exists and honours the value. A repeating binding sends a LITERAL
# uppercase J into the pane, so prefix+j then j then j prints "JJJ" while the
# binding keeps repeating and "Jjj" once the client has left the prefix table
# (the bare j falls through to the shell as a lowercase j).
# ---------------------------------------------------------------------------
Write-Host "`n=== Part E: attached client, injected keystrokes ===" -ForegroundColor Cyan
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$keyInj = Join-Path $TMP "keys606.exe"
if (-not (Test-Path $keyInj)) {
    & $csc /nologo /optimize /out:$keyInj (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
}
if (-not (Test-Path $keyInj)) {
    Write-Fail "could not compile tests/injector.cs, skipping the TUI section"
} else {
    $SE = "t606e"
    # The reporter's scenario: repeat-time comes from the config file, not the
    # CLI. On the old binary this line warned and was dropped.
    $tuiConf = Join-Path $TMP "repeat_tui.conf"
    "set -g mouse off`nset -g repeat-time 0" | Set-Content -Path $tuiConf -Encoding ASCII
    $launchCmd = Join-Path $TMP "launch606.cmd"
    @"
@echo off
set PSMUX_SESSION=
set PSMUX_SESSION_NAME=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
set NO_COLOR=
set PSMUX_NO_WARM=1
set PSMUX_DATA_DIR=$dataDir
"$PSMUX" -L $SOCK -f "$tuiConf" new-session -s %1 -x 120 -y 30 cmd
"@ | Set-Content -Path $launchCmd -Encoding ASCII

    Kill-Sess $SE
    $null = Start-Process -FilePath $launchCmd -ArgumentList $SE -PassThru
    for ($i = 0; $i -lt 100; $i++) {
        if ((Test-Path "$dataDir\${SOCK}__$SE.port") -or (Test-Path "$dataDir\$SE.port")) { break }
        Start-Sleep -Milliseconds 250
    }
    Start-Sleep -Seconds 5
    $cli = Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" |
        Where-Object { $_.CommandLine -match "new-session -s\s+$SE\b" } | Select-Object -First 1
    if (-not $cli) {
        Write-Fail "attached client did not start, skipping the TUI section"
    } else {
        $cpid = [int]$cli.ProcessId
        Write-Info "attached client pid=$cpid"
        & $PSMUX -L $SOCK bind-key -r j send-keys -l J 2>&1 | Out-Null
        Start-Sleep -Milliseconds 500

        function Probe([int]$gapMs) {
            & $PSMUX -L $SOCK send-keys -t $SE Enter 2>&1 | Out-Null
            Start-Sleep -Milliseconds 900
            & $keyInj $cpid "^b{SLEEP:400}j{SLEEP:$gapMs}j{SLEEP:$gapMs}j" 2>&1 | Out-Null
            Start-Sleep -Milliseconds 1600
            $after = (& $PSMUX -L $SOCK capture-pane -t $SE -p 2>&1 | Out-String)
            $lines = ($after -split "`r?`n") | Where-Object { $_ -match '\S' }
            $last = if ($lines) { $lines[-1] } else { "" }
            return ([regex]::Match($last, '[Jj]+\s*$')).Value.Trim()
        }

        # E1: the config said 0, so the binding must NOT repeat.
        $cfgVal = Show1 $SE
        if ($cfgVal -eq "repeat-time 0") { Write-Pass "config-set repeat-time reached the live session: '$cfgVal'" }
        else { Write-Fail "config-set repeat-time missing: '$cfgVal', expected 'repeat-time 0'" }
        # -ceq throughout: PowerShell's -eq is case insensitive, so 'JJJ' -eq
        # 'Jjj' is true and every one of these checks would pass vacuously.
        $r0 = Probe 200
        Write-Info "repeat-time 0 (from config), 200ms gap -> tail '$r0'"
        if ($r0 -ceq "Jjj") { Write-Pass "repeat-time 0 disables bind-key -r repeat (tail 'Jjj')" }
        elseif ($r0 -ceq "JJJ") { Write-Fail "BUG: repeat-time 0 from the config was ignored, the key still repeated (tail 'JJJ')" }
        else { Write-Fail "inconclusive injection, tail '$r0' (expected Jjj or JJJ)" }

        # E2: default 500 ms with a short gap must repeat.
        & $PSMUX -L $SOCK set-option -g repeat-time 500 2>&1 | Out-Null
        Start-Sleep -Milliseconds 600
        $rDef = Probe 200
        Write-Info "repeat-time 500, 200ms gap -> tail '$rDef'"
        if ($rDef -ceq "JJJ") { Write-Pass "default repeat window repeats within 200ms (tail 'JJJ')" }
        else { Write-Fail "default repeat window did not repeat, tail '$rDef'" }

        # E3: default 500 ms with a 1500 ms gap must time out.
        $rSlow = Probe 1500
        Write-Info "repeat-time 500, 1500ms gap -> tail '$rSlow'"
        if ($rSlow -ceq "Jjj") { Write-Pass "500ms window expires before a 1500ms gap (tail 'Jjj')" }
        else { Write-Fail "500ms window did not expire over 1500ms, tail '$rSlow'" }

        # E4: a long window must survive the same gap.
        & $PSMUX -L $SOCK set-option -g repeat-time 3000 2>&1 | Out-Null
        Start-Sleep -Milliseconds 600
        $rLong = Probe 1500
        Write-Info "repeat-time 3000, 1500ms gap -> tail '$rLong'"
        if ($rLong -ceq "JJJ") { Write-Pass "3000ms window survives a 1500ms gap (tail 'JJJ')" }
        else { Write-Fail "3000ms window did not survive a 1500ms gap, tail '$rLong'" }

        # E5: the TUI command prompt route (prefix : set -g repeat-time N).
        # E4 left a 3000ms repeat window armed; while the client is still in
        # the prefix table the next prefix press is the send-prefix binding,
        # not the gateway to `:`. Wait the window out first.
        & $PSMUX -L $SOCK set-option -g repeat-time 500 2>&1 | Out-Null
        Start-Sleep -Milliseconds 3500
        # Opening the prompt, typing and submitting go in separate injector
        # runs: the prompt has to be painted before the first character or the
        # whole line lands in the pane instead.
        & $keyInj $cpid "^b{SLEEP:800}:" 2>&1 | Out-Null
        Start-Sleep -Milliseconds 1200
        & $keyInj $cpid "set -g repeat-time 1900" 2>&1 | Out-Null
        Start-Sleep -Milliseconds 1200
        & $keyInj $cpid "{ENTER}" 2>&1 | Out-Null
        Start-Sleep -Milliseconds 1500
        $viaPrompt = Show1 $SE
        if ($viaPrompt -eq "repeat-time 1900") { Write-Pass "command prompt set it: '$viaPrompt'" }
        else { Write-Fail "command prompt route -> '$viaPrompt', expected 'repeat-time 1900'" }

        try { Stop-Process -Id $cpid -Force -EA SilentlyContinue } catch {}
    }
    Kill-Sess $SE
}

Write-Host "`n=== Issue #606 results: $script:Pass passed, $script:Fail failed ===" -ForegroundColor Cyan
if ($script:Fail -gt 0) { exit 1 }
exit 0
