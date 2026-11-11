# Issue #536: quoted whitespace in config-file `set-option` values was collapsed.
#
# `parse_set_option` tokenized the whole line with split_whitespace() and
# rejoined the value with single spaces, so quoting could not protect a run of
# spaces. The CLI path was unaffected because main.rs re-quotes an argument
# before sending it and the control tokenizer honours those quotes. Same
# command, two different results, no error either way.
#
# Expected behaviour is taken from tmux 3.4, measured directly (WSL), not
# assumed. tmux keeps a quoted value byte exact:
#   set -g @x "A     B"          -> 7 chars
#   set -g @x "   leading"       -> 10 chars
#   set -g @x "trailing   "      -> 11 chars
#   set -g @x "left<12sp>right"  -> 21 chars
#   set -g @x "say \"hi\""       -> 8 chars  (escapes processed)
#   set -g @x ""                 -> 0 chars
#
# Every assertion here compares an exact character count, because the whole
# failure mode is invisible to the eye: the bar still looks plausible.

# The runner console is OEM 437: read psmux output as UTF-8 so non-ASCII cells compare by their real bytes.
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$SESSION = "test_issue536"
$psmuxDir = "$env:USERPROFILE\.psmux"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

function Cleanup {
    & $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue
}

# Read an option back EXACTLY: strip only the trailing newline that the CLI
# adds, never internal or leading/trailing spaces (which are the thing on test).
function Get-Opt([string]$name, [string]$Sess = $SESSION) {
    $raw = & $PSMUX show -gv $name -t $Sess 2>&1 | Out-String
    return $raw -replace "`r?`n$",""
}

# Render a value as a visible code sequence so a failure message is readable.
function Viz([string]$s) {
    if ($null -eq $s) { return "<null>" }
    $out = ($s.ToCharArray() | ForEach-Object {
        $c = [int]$_
        if ($c -eq 32) { "SP" } elseif ($c -eq 9) { "TAB" }
        elseif ($c -lt 32) { "\x{0:X2}" -f $c }
        elseif ($c -eq 160) { "NBSP" }
        elseif ($c -gt 126) { "U+{0:X4}" -f $c }
        else { [string]$_ }
    }) -join " "
    return "len=$($s.Length) [$out]"
}

# Write a config file with exact bytes: no BOM, LF endings, nothing normalised.
function Write-Conf([string]$path, [string[]]$lines) {
    $enc = [System.Text.UTF8Encoding]::new($false)
    [System.IO.File]::WriteAllText($path, (($lines -join "`n") + "`n"), $enc)
}

function Assert-Len($label, $actual, $expected) {
    if ($null -ne $actual -and $actual.Length -eq $expected) {
        Write-Pass "$label -> $(Viz $actual)"
    } else {
        Write-Fail "$label -> expected len=$expected, got $(Viz $actual)"
    }
}

Cleanup
& $PSMUX new-session -d -s $SESSION
Start-Sleep -Seconds 3
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) { Write-Host "[FATAL] session creation failed" -ForegroundColor Red; exit 1 }

Write-Host "`n=== Issue #536: quoted whitespace in config values ===" -ForegroundColor Cyan

# --- Test 1: the reported reproduction ---------------------------------------
Write-Host "`n[Test 1] Reported repro: quoted gap must survive source-file" -ForegroundColor Yellow
$conf = "$env:TEMP\psmux_536_gap.conf"
Write-Conf $conf @(
    'set -g @gap_quoted "A     B"',
    "set -g @gap_single 'A     B'",
    'set -g @gap_lead "   leading"',
    'set -g @gap_trail "trailing   "',
    'set -g @gap_status "left            right"'
)
& $PSMUX source-file $conf -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 700

Assert-Len 'set -g @gap_quoted "A     B"'          (Get-Opt "@gap_quoted") 7
Assert-Len "set -g @gap_single 'A     B'"          (Get-Opt "@gap_single") 7
Assert-Len 'set -g @gap_lead "   leading"'         (Get-Opt "@gap_lead")   10
Assert-Len 'set -g @gap_trail "trailing   "'       (Get-Opt "@gap_trail")  11
Assert-Len 'set -g @gap_status "left<12sp>right"'  (Get-Opt "@gap_status") 21

# Content, not just length: the gap must be spaces in the right place.
$q = Get-Opt "@gap_quoted"
if ($q -eq "A     B") { Write-Pass "Quoted value is byte exact ('A' + 5 spaces + 'B')" }
else { Write-Fail "Quoted value wrong: $(Viz $q)" }

# --- Test 2: CLI and config must now agree -----------------------------------
# This divergence was the actual complaint: the same command, two results.
Write-Host "`n[Test 2] CLI path and config path produce identical values" -ForegroundColor Yellow
$optName = "@gap_cli"
& $PSMUX set -g $optName "A     B" -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 400
$cliVal = Get-Opt "@gap_cli"
$cfgVal = Get-Opt "@gap_quoted"
Assert-Len "CLI set -g @gap_cli 'A     B'" $cliVal 7
if ($cliVal -eq $cfgVal) { Write-Pass "CLI value == config value ('$($cliVal.Length)' chars both)" }
else { Write-Fail "Divergence remains: CLI $(Viz $cliVal) vs config $(Viz $cfgVal)" }

# --- Test 3: a real built-in option, not just @user options ------------------
# status-right with a deliberate gap is the reported real-world symptom.
Write-Host "`n[Test 3] status-right gap from config (the reported symptom)" -ForegroundColor Yellow
$confSr = "$env:TEMP\psmux_536_sr.conf"
Write-Conf $confSr @('set -g status-right "left            right"')
& $PSMUX source-file $confSr -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 600
$srCfg = Get-Opt "status-right"
Assert-Len "status-right from CONFIG" $srCfg 21

& $PSMUX set -g status-right "left            right" -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 400
$srCli = Get-Opt "status-right"
Assert-Len "status-right from CLI" $srCli 21
if ($srCfg -eq $srCli) { Write-Pass "status-right identical from both paths" }
else { Write-Fail "status-right still diverges: cfg $(Viz $srCfg) vs cli $(Viz $srCli)" }

# --- Test 4: the width-matching case from the report -------------------------
# Three branches padded to equal width; the idle branch is three spaces and
# collapsed to one, shoving the bar sideways when the prefix was held.
Write-Host "`n[Test 4] Equal-width branches keep their padding" -ForegroundColor Yellow
$confW = "$env:TEMP\psmux_536_width.conf"
Write-Conf $confW @('set -g @width_ind "#{?client_prefix, X ,#{?pane_in_mode, Y ,   }}"')
& $PSMUX source-file $confW -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
$w = Get-Opt "@width_ind"
if ($w -match '\{\?pane_in_mode, Y ,   \}\}$') {
    Write-Pass "Idle branch kept its three spaces -> $(Viz $w)"
} else {
    Write-Fail "Idle branch padding lost -> $(Viz $w)"
}

# --- Test 5: startup config path, not only source-file ----------------------
Write-Host "`n[Test 5] Config applied at session startup (PSMUX_CONFIG_FILE)" -ForegroundColor Yellow
$S2 = "test_issue536_boot"
& $PSMUX kill-session -t $S2 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
Remove-Item "$psmuxDir\$S2.*" -Force -EA SilentlyContinue
$env:PSMUX_CONFIG_FILE = $conf
& $PSMUX new-session -d -s $S2
Start-Sleep -Seconds 3
$env:PSMUX_CONFIG_FILE = $null
& $PSMUX has-session -t $S2 2>$null
if ($LASTEXITCODE -eq 0) {
    Assert-Len "@gap_quoted at STARTUP" (Get-Opt "@gap_quoted" $S2) 7
} else {
    Write-Fail "startup session did not come up"
}
& $PSMUX kill-session -t $S2 2>&1 | Out-Null
Remove-Item "$psmuxDir\$S2.*" -Force -EA SilentlyContinue

# --- Test 6: non-ASCII whitespace through the CLI ----------------------------
# main.rs decided whether to re-quote with s.contains(' '), so a value whose
# only separators were NBSPs went unquoted, got re-split by the control
# tokenizer, and came back collapsed AND rewritten to ASCII spaces.
Write-Host "`n[Test 6] Non-ASCII whitespace survives the CLI" -ForegroundColor Yellow
$NBSP = [char]0x00A0
$u1 = "A" + $NBSP + $NBSP + $NBSP + "B"           # NBSP only, no ASCII space
& $PSMUX set -g "@u1" $u1 -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 400
$got1 = Get-Opt "@u1"
if ($got1 -eq $u1) { Write-Pass "NBSP-only value byte exact -> $(Viz $got1)" }
else { Write-Fail "NBSP-only value mangled: sent $(Viz $u1) got $(Viz $got1)" }

$u2 = "A" + $NBSP + $NBSP + " " + $NBSP + $NBSP + "B"   # mixed NBSP + ASCII
& $PSMUX set -g "@u2" $u2 -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 400
$got2 = Get-Opt "@u2"
if ($got2 -eq $u2) { Write-Pass "Mixed NBSP/ASCII value byte exact -> $(Viz $got2)" }
else { Write-Fail "Mixed value mangled: sent $(Viz $u2) got $(Viz $got2)" }

# --- Test 7: REGRESSION GUARDS ----------------------------------------------
# Everything the old split_whitespace path got right must still be right.
Write-Host "`n[Test 7] Regression guards for existing config behaviour" -ForegroundColor Yellow
$confR = "$env:TEMP\psmux_536_regress.conf"
Write-Conf $confR @(
    'set -g @r_plain bar',
    'set -g @r_comment bar # trailing comment',
    'set -g @r_hashval "#{session_name} x"',
    'set -g @r_style "#[fg=red]TXT#[default]"',
    'set -g @r_pct "%H:%M %d-%b"',
    'set -g @r_semi "a;b"',
    'set -g @r_empty ""',
    'set -g @r_inner a"b"c',
    'set -g @r_multiword one two three',
    'set -g @r_trailws bar   ',
    'set -g @r_esc "say \"hi\""'
)
& $PSMUX source-file $confR -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 800

$expect = @(
    @{ n="@r_plain";     v="bar" },
    @{ n="@r_comment";   v="bar" },                      # inline comment stripped (#416)
    @{ n="@r_hashval";   v="#{session_name} x" },        # format not treated as comment
    @{ n="@r_style";     v="#[fg=red]TXT#[default]" },   # style run intact
    @{ n="@r_pct";       v="%H:%M %d-%b" },
    @{ n="@r_semi";      v="a;b" },                      # quoted ; not a splitter (#499)
    @{ n="@r_empty";     v="" },
    @{ n="@r_inner";     v='a"b"c' },                    # inner quotes are content
    @{ n="@r_multiword"; v="one two three" },            # bare words still join
    @{ n="@r_trailws";   v="bar" }                       # unquoted trailing ws trimmed
)
foreach ($e in $expect) {
    $got = Get-Opt $e.n
    if ($got -eq $e.v) { Write-Pass "$($e.n) = '$($e.v)'" }
    else { Write-Fail "$($e.n) expected '$($e.v)' got $(Viz $got)" }
}
# Escapes are now processed inside double quotes, matching tmux (was: backslashes kept).
$esc = Get-Opt "@r_esc"
if ($esc -eq 'say "hi"') { Write-Pass "@r_esc unescaped to 'say `"hi`"' (tmux parity)" }
else { Write-Fail "@r_esc expected 'say `"hi`"' got $(Viz $esc)" }

# --- Test 8: a config format value still renders --------------------------
Write-Host "`n[Test 8] A stored format value still expands when rendered" -ForegroundColor Yellow
$confF = "$env:TEMP\psmux_536_fmt.conf"
Write-Conf $confF @('set -g status-right "#{session_name}  END"')
& $PSMUX source-file $confF -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
$stored = Get-Opt "status-right"
if ($stored -eq "#{session_name}  END") { Write-Pass "Format stored verbatim with its double space" }
else { Write-Fail "Format storage wrong: $(Viz $stored)" }
$rendered = (& $PSMUX display-message -t $SESSION -p '#{status-right}' 2>&1 | Out-String) -replace "`r?`n$",""
if ($rendered -match "END") { Write-Pass "Stored format still renders ('$rendered')" }
else { Write-Fail "Stored format did not render: [$rendered]" }

# --- Test 9: TCP server path -------------------------------------------------
Write-Host "`n[Test 9] Raw TCP path preserves a quoted gap" -ForegroundColor Yellow
$port = (Get-Content "$psmuxDir\$SESSION.port" -Raw).Trim()
$key  = (Get-Content "$psmuxDir\$SESSION.key" -Raw).Trim()
function Send-Tcp([string]$cmd) {
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.NoDelay = $true
    $st = $tcp.GetStream()
    $w = [System.IO.StreamWriter]::new($st); $rd = [System.IO.StreamReader]::new($st)
    $w.Write("AUTH $key`n"); $w.Flush()
    if ($rd.ReadLine() -ne "OK") { $tcp.Close(); return "AUTH_FAILED" }
    $w.Write("$cmd`n"); $w.Flush()
    $st.ReadTimeout = 5000
    try { $resp = $rd.ReadLine() } catch { $resp = "" }
    $tcp.Close()
    return $resp
}
$null = Send-Tcp 'set -g @tcp_gap "A     B"'
Start-Sleep -Milliseconds 500
Assert-Len "TCP set -g @tcp_gap" (Get-Opt "@tcp_gap") 7

# --- Test 10: Win32 TUI visual verification ---------------------------------
Write-Host "`n[Test 10] Win32 TUI visual verification" -ForegroundColor Yellow
$SESSION_TUI = "test536_tui"
& $PSMUX kill-session -t $SESSION_TUI 2>&1 | Out-Null
Remove-Item "$psmuxDir\$SESSION_TUI.*" -Force -EA SilentlyContinue
$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION_TUI -PassThru
Start-Sleep -Seconds 4

& $PSMUX source-file $confSr -t $SESSION_TUI 2>&1 | Out-Null
Start-Sleep -Milliseconds 800
$tuiSr = Get-Opt "status-right" $SESSION_TUI
Assert-Len "TUI: status-right gap applied to a live attached session" $tuiSr 21

& $PSMUX has-session -t $SESSION_TUI 2>$null
if ($LASTEXITCODE -eq 0) { Write-Pass "TUI: session healthy after config reload" }
else { Write-Fail "TUI: session died after config reload" }

$panesBefore = ((& $PSMUX display-message -t $SESSION_TUI -p '#{window_panes}' 2>&1) | Out-String).Trim()
& $PSMUX split-window -v -t $SESSION_TUI 2>&1 | Out-Null
Start-Sleep -Milliseconds 800
$panesAfter = ((& $PSMUX display-message -t $SESSION_TUI -p '#{window_panes}' 2>&1) | Out-String).Trim()
if ($panesBefore -eq "1" -and $panesAfter -eq "2") { Write-Pass "TUI: still functional (split 1 -> 2 panes)" }
else { Write-Fail "TUI: split failed, panes '$panesBefore' -> '$panesAfter'" }

& $PSMUX kill-session -t $SESSION_TUI 2>&1 | Out-Null
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
Remove-Item "$psmuxDir\$SESSION_TUI.*" -Force -EA SilentlyContinue

Cleanup
Remove-Item "$env:TEMP\psmux_536_*" -Force -EA SilentlyContinue

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
