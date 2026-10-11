<#
    test_combined_flags.ps1
    Proves that combined flags (-ga, -gu, -gq, -gqv) and separated flags (-g -a, -u, -q)
    both work identically across TCP and CLI channels.

    Every test uses SET + VERIFY: set a value, then independently verify it changed
    using show-options -gqv (value-only mode) or show-hooks/show-environment.

    Uses: TCP (persistent handler at connection.rs:2330+) and CLI binary (session.rs send_control).
#>

$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\tcp_reply_common.ps1"
$pass = 0; $fail = 0; $skip = 0; $total = 0

function Write-Test($msg) { Write-Host "  TEST: $msg" -ForegroundColor Cyan; $script:total++ }
function Write-Pass($msg) { Write-Host "  PASS: $msg" -ForegroundColor Green; $script:pass++ }
function Write-Fail($msg) { Write-Host "  FAIL: $msg" -ForegroundColor Red; $script:fail++ }
function Write-Skip($msg) { Write-Host "  SKIP: $msg" -ForegroundColor Yellow; $script:skip++ }

function Test-Result($name, $cond, $detail) {
    $script:total++
    if ($cond) { Write-Pass "$name ($detail)" } else { Write-Fail "$name ($detail)" }
}

# ---------------------------------------------------------------
# Session setup
# ---------------------------------------------------------------
$psmux = (Get-Command psmux -ErrorAction SilentlyContinue).Source
if (-not $psmux) { $psmux = (Get-Command tmux -ErrorAction SilentlyContinue).Source }
if (-not $psmux) { Write-Host "psmux not found"; exit 1 }

$SESSION = "cflagtest_" + (Get-Random -Maximum 9999)
Write-Host "Creating session: $SESSION" -ForegroundColor Magenta
& $psmux new-session -d -s $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 2

$portFile = "$env:USERPROFILE\.psmux\$SESSION.port"
$keyFile  = "$env:USERPROFILE\.psmux\$SESSION.key"
if (-not (Test-Path $portFile) -or -not (Test-Path $keyFile)) {
    Write-Host "Session port/key files not found"; exit 1
}

$port = (Get-Content $portFile).Trim()
$key  = (Get-Content $keyFile).Trim()
Write-Host "Session $SESSION on port $port`n" -ForegroundColor Magenta

# ---------------------------------------------------------------
# TCP helper with proper AUTH
# ---------------------------------------------------------------
function Send-Tcp($cmd) {
    $client = New-Object System.Net.Sockets.TcpClient("127.0.0.1", [int]$port)
    $client.ReceiveTimeout = 3000
    $stream = $client.GetStream()
    $writer = New-Object System.IO.StreamWriter($stream)
    $reader = New-Object System.IO.StreamReader($stream)
    $writer.AutoFlush = $true
    $writer.WriteLine("AUTH $key")
    $auth = $reader.ReadLine()
    if ($auth -ne "OK") { $client.Close(); return @{ok=$false; resp="AUTH failed: $auth"} }
    $writer.WriteLine($cmd)
    # The reply ends when the server closes the connection (tcp_reply_common.ps1).
    $resp = Read-TcpReplyToEof -Stream $stream -TimeoutMs 3000
    $client.Close()
    @{ok=$true; resp=$resp.Trim()}
}

# CLI helpers
function CLI-Set($extraArgs) { & $psmux set-option -t $SESSION @extraArgs 2>&1 | Out-Null }
function CLI-Show($option) { (& $psmux show-options -gqv -t $SESSION $option 2>&1 | Out-String).Trim() }
function CLI-ShowAll($option) { (& $psmux show-options -t $SESSION $option 2>&1 | Out-String).Trim() }

# ================================================================
# SECTION 1: TCP combined flags (persistent handler)
# ================================================================
Write-Host "=== 1. TCP COMBINED FLAGS ===" -ForegroundColor Cyan

# -ga (combined append) on set-option
Write-Test "TCP -ga set-option append"
Send-Tcp "set-option -g status-right TCP_A" | Out-Null
Send-Tcp "set-option -ga status-right TCP_B" | Out-Null
$r = Send-Tcp "show-options -gqv status-right"
Test-Result "tcp-combined-ga-append" ($r.ok -and $r.resp -eq 'TCP_ATCP_B') "resp='$($r.resp)'"

# -gu (combined unset) on set-option
Write-Test "TCP -gu set-option unset"
Send-Tcp "set-option -g @tcp-gu-test PRESENT" | Out-Null
$before = Send-Tcp "show-options -gqv @tcp-gu-test"
Send-Tcp "set-option -gu @tcp-gu-test" | Out-Null
$after = Send-Tcp "show-options -gqv @tcp-gu-test"
Test-Result "tcp-combined-gu-before" ($before.ok -and $before.resp -eq 'PRESENT') "before='$($before.resp)'"
Test-Result "tcp-combined-gu-after" ($after.ok -and $after.resp -ne 'PRESENT') "after='$($after.resp)'"

# -gq (combined quiet) on set-option
Write-Test "TCP -gq set-option quiet"
Send-Tcp "set-option -gq totally-nonexistent-option qval" | Out-Null
$r = Send-Tcp "show-options -gqv mouse"
Test-Result "tcp-combined-gq-quiet" ($r.ok -and $r.resp -match 'on|off') "mouse='$($r.resp)'"

# -au (combined append+unset, unset wins per tmux) on user option
Write-Test "TCP -au set-option"
Send-Tcp "set-option -g @tcp-au au_val" | Out-Null
Send-Tcp "set-option -au @tcp-au" | Out-Null
$r = Send-Tcp "show-options -gqv @tcp-au"
Test-Result "tcp-combined-au-unset" ($r.ok -and $r.resp -ne 'au_val') "resp='$($r.resp)'"

# -qa (combined quiet+append)
Write-Test "TCP -qa set-option quiet+append"
Send-Tcp "set-option -g status-left QA1" | Out-Null
Send-Tcp "set-option -qa status-left QA2" | Out-Null
$r = Send-Tcp "show-options -gqv status-left"
Test-Result "tcp-combined-qa-append" ($r.ok -and $r.resp -eq 'QA1QA2') "resp='$($r.resp)'"

# -go (combined only-if-unset: existing value preserved)
Write-Test "TCP -go set-option only-if-unset (existing)"
Send-Tcp "set-option -g escape-time 42" | Out-Null
Send-Tcp "set-option -go escape-time 999" | Out-Null
$r = Send-Tcp "show-options -gqv escape-time"
Test-Result "tcp-combined-go-existing" ($r.ok -and $r.resp -eq '42') "resp='$($r.resp)'"

# -go (combined only-if-unset: new @option gets set)
Write-Test "TCP -go set-option only-if-unset (new)"
Send-Tcp "set-option -u @go-new-test" | Out-Null
Send-Tcp "set-option -go @go-new-test first" | Out-Null
$r = Send-Tcp "show-options -gqv @go-new-test"
Test-Result "tcp-combined-go-new" ($r.ok -and $r.resp -eq 'first') "resp='$($r.resp)'"
Send-Tcp "set-option -g escape-time 500" | Out-Null

# ================================================================
# SECTION 2: TCP separated flags (regression: must still work)
# ================================================================
Write-Host "`n=== 2. TCP SEPARATED FLAGS ===" -ForegroundColor Cyan

# -a (separated append)
Write-Test "TCP separated -a append"
Send-Tcp "set-option -g status-right SEP_A" | Out-Null
Send-Tcp "set-option -a status-right SEP_B" | Out-Null
$r = Send-Tcp "show-options -gqv status-right"
Test-Result "tcp-separated-a-append" ($r.ok -and $r.resp -eq 'SEP_ASEP_B') "resp='$($r.resp)'"

# -u (separated unset)
Write-Test "TCP separated -u unset"
Send-Tcp "set-option -g @tcp-sep-u HERE" | Out-Null
$before = Send-Tcp "show-options -gqv @tcp-sep-u"
Send-Tcp "set-option -u @tcp-sep-u" | Out-Null
$after = Send-Tcp "show-options -gqv @tcp-sep-u"
Test-Result "tcp-separated-u-before" ($before.ok -and $before.resp -eq 'HERE') "before='$($before.resp)'"
Test-Result "tcp-separated-u-after" ($after.ok -and $after.resp -ne 'HERE') "after='$($after.resp)'"

# -q (separated quiet)
Write-Test "TCP separated -q quiet"
Send-Tcp "set-option -q nonexistent-junk-option val" | Out-Null
$r = Send-Tcp "show-options -gqv mouse"
Test-Result "tcp-separated-q-quiet" ($r.ok -and $r.resp -match 'on|off') "mouse='$($r.resp)'"

# ================================================================
# SECTION 3: CLI combined flags
# ================================================================
Write-Host "`n=== 3. CLI COMBINED FLAGS ===" -ForegroundColor Cyan

# -ga via CLI
Write-Test "CLI -ga append"
CLI-Set @('-g', 'status-right', 'CLI_A')
& $psmux set-option -ga status-right CLI_B -t $SESSION 2>&1 | Out-Null
$r = CLI-Show 'status-right'
Test-Result "cli-combined-ga-append" ($r -eq 'CLI_ACLI_B') "resp='$r'"

# -gu via CLI
Write-Test "CLI -gu unset"
CLI-Set @('-g', '@cli-gu-test', 'PRESENT')
$before = CLI-Show '@cli-gu-test'
& $psmux set-option -gu @cli-gu-test -t $SESSION 2>&1 | Out-Null
$after = CLI-Show '@cli-gu-test'
Test-Result "cli-combined-gu-before" ($before -eq 'PRESENT') "before='$before'"
Test-Result "cli-combined-gu-after" ($after -ne 'PRESENT') "after='$after'"

# -gq via CLI
Write-Test "CLI -gq quiet"
& $psmux set-option -gq totally-nonexistent val -t $SESSION 2>&1 | Out-Null
$r = CLI-Show 'mouse'
Test-Result "cli-combined-gq-quiet" ($r -match 'on|off') "mouse='$r'"

# ================================================================
# SECTION 4: CLI separated flags (regression)
# ================================================================
Write-Host "`n=== 4. CLI SEPARATED FLAGS ===" -ForegroundColor Cyan

# -a via CLI
Write-Test "CLI separated -a append"
CLI-Set @('-g', 'status-right', 'CS_A')
& $psmux set-option -a status-right CS_B -t $SESSION 2>&1 | Out-Null
$r = CLI-Show 'status-right'
Test-Result "cli-separated-a-append" ($r -eq 'CS_ACS_B') "resp='$r'"

# -u via CLI
Write-Test "CLI separated -u unset"
CLI-Set @('-g', '@cli-sep-u', 'HERE')
$before = CLI-Show '@cli-sep-u'
& $psmux set-option -u @cli-sep-u -t $SESSION 2>&1 | Out-Null
$after = CLI-Show '@cli-sep-u'
Test-Result "cli-separated-u-before" ($before -eq 'HERE') "before='$before'"
Test-Result "cli-separated-u-after" ($after -ne 'HERE') "after='$after'"

# ================================================================
# SECTION 5: show-options combined flags (already worked, regression)
# ================================================================
Write-Host "`n=== 5. SHOW-OPTIONS COMBINED FLAGS ===" -ForegroundColor Cyan

# -gqv via TCP
Write-Test "TCP show-options -gqv"
Send-Tcp "set-option -g @so-gqv SHOWVAL" | Out-Null
$r = Send-Tcp "show-options -gqv @so-gqv"
Test-Result "tcp-show-gqv" ($r.ok -and $r.resp -eq 'SHOWVAL') "resp='$($r.resp)'"

# -gqv via CLI
Write-Test "CLI show-options -gqv"
CLI-Set @('-g', '@so-cli', 'CLIVAL')
$r = (& $psmux show-options -gqv -t $SESSION @so-cli 2>&1 | Out-String).Trim()
Test-Result "cli-show-gqv" ($r -eq 'CLIVAL') "resp='$r'"

# -gv via TCP (without quiet)
Write-Test "TCP show-options -gv"
Send-Tcp "set-option -g @so-gv GVVAL" | Out-Null
$r = Send-Tcp "show-options -gv @so-gv"
Test-Result "tcp-show-gv" ($r.ok -and $r.resp -eq 'GVVAL') "resp='$($r.resp)'"

# -Av (all options + value only)
Write-Test "TCP show-options -Av"
Send-Tcp "set-option -g @show-av AVTEST" | Out-Null
$r = Send-Tcp "show-options -Av @show-av"
Test-Result "tcp-show-Av" ($r.ok -and $r.resp -match 'AVTEST') "resp='$($r.resp)'"

# ================================================================
# SECTION 6: set-hook combined flags
# ================================================================
Write-Host "`n=== 6. SET-HOOK COMBINED FLAGS ===" -ForegroundColor Cyan

# -ga (combined append) via CLI
Write-Test "CLI set-hook -ga append"
$env:PSMUX_TARGET_SESSION = $SESSION
& $psmux set-hook after-new-window 'run echo hookA' 2>&1 | Out-Null
& $psmux set-hook -ga after-new-window 'run echo hookB' 2>&1 | Out-Null
$hooks = (& $psmux show-hooks -t $SESSION 2>&1 | Out-String).Trim()
Test-Result "cli-hook-ga-append" ($hooks -match 'hookA' -and $hooks -match 'hookB') "hooks='$hooks'"

# -gu (combined unset) via CLI
Write-Test "CLI set-hook -gu unset"
& $psmux set-hook -gu after-new-window -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 200
$hooks2 = (& $psmux show-hooks -t $SESSION 2>&1 | Out-String).Trim()
Test-Result "cli-hook-gu-unset" ($hooks2 -notmatch 'hookA') "hooks='$hooks2'"

# Separated -a for hooks via CLI
Write-Test "CLI set-hook separated -a"
& $psmux set-hook after-split-window 'run echo sepA' 2>&1 | Out-Null
& $psmux set-hook -a after-split-window 'run echo sepB' 2>&1 | Out-Null
$hooks3 = (& $psmux show-hooks -t $SESSION 2>&1 | Out-String).Trim()
Test-Result "cli-hook-sep-a" ($hooks3 -match 'sepA' -and $hooks3 -match 'sepB') "hooks='$hooks3'"

# ================================================================
# SECTION 7: set-environment combined flags
# ================================================================
Write-Host "`n=== 7. SET-ENVIRONMENT COMBINED FLAGS ===" -ForegroundColor Cyan

# -gu (combined unset) via TCP
Write-Test "TCP setenv + unsetenv -gu"
# -gu removes the GLOBAL value (tmux cmd-set-environment.c), so the variable
# is set and read back in the global environment (#775).
Send-Tcp "set-environment -g CFTEST_VAR myval" | Out-Null
$env1 = Send-Tcp "show-environment -g"
Test-Result "tcp-env-set" ($env1.ok -and $env1.resp -match 'CFTEST_VAR=myval') "env='$($env1.resp)'"
Send-Tcp "set-environment -gu CFTEST_VAR" | Out-Null
$env2 = Send-Tcp "show-environment -g"
Test-Result "tcp-env-gu-unset" ($env2.ok -and $env2.resp -notmatch 'CFTEST_VAR') "env='$($env2.resp)'"

# Separated -u via TCP
Write-Test "TCP setenv + unsetenv separated -u"
Send-Tcp "set-environment CFTEST2 val2" | Out-Null
Send-Tcp "set-environment -u CFTEST2" | Out-Null
$env3 = Send-Tcp "show-environment"
Test-Result "tcp-env-sep-u" ($env3.ok -and $env3.resp -notmatch 'CFTEST2') "env='$($env3.resp)'"

# ================================================================
# SECTION 8: Cross-channel verification (set via TCP combined, verify via CLI)
# ================================================================
Write-Host "`n=== 8. CROSS-CHANNEL VERIFICATION ===" -ForegroundColor Cyan

# Set combined -ga via TCP, verify via CLI
Write-Test "TCP -ga -> CLI verify"
Send-Tcp "set-option -g status-right CROSS_A" | Out-Null
Send-Tcp "set-option -ga status-right CROSS_B" | Out-Null
$r = CLI-Show 'status-right'
Test-Result "cross-tcp-ga-to-cli" ($r -eq 'CROSS_ACROSS_B') "cli='$r'"

# Set combined -ga via CLI, verify via TCP
Write-Test "CLI -ga -> TCP verify"
CLI-Set @('-g', 'status-left', 'REV_A')
& $psmux set-option -ga status-left REV_B -t $SESSION 2>&1 | Out-Null
$r = Send-Tcp "show-options -gqv status-left"
Test-Result "cross-cli-ga-to-tcp" ($r.ok -and $r.resp -eq 'REV_AREV_B') "tcp='$($r.resp)'"

# Set combined -gu via TCP, verify via CLI
Write-Test "TCP -gu -> CLI verify"
Send-Tcp "set-option -g @cross-gu XVAL" | Out-Null
Send-Tcp "set-option -gu @cross-gu" | Out-Null
$r = CLI-Show '@cross-gu'
Test-Result "cross-tcp-gu-to-cli" ($r -ne 'XVAL') "cli='$r'"

# ================================================================
# SECTION 9: Config parser combined flags (Rust unit test references)
# ================================================================
Write-Host "`n=== 9. CONFIG PARSER COMBINED FLAGS (via source-file) ===" -ForegroundColor Cyan

$cfgFile = "$env:TEMP\cflag_test_$SESSION.conf"

# -ga in config file
Write-Test "Config -ga via source-file"
"set -g status-right CFG_A`nset -ga status-right CFG_B" | Set-Content $cfgFile -Encoding UTF8
Send-Tcp "source-file `"$cfgFile`"" | Out-Null
Start-Sleep -Milliseconds 500
$r = Send-Tcp "show-options -gqv status-right"
Test-Result "cfg-combined-ga" ($r.ok -and $r.resp -eq 'CFG_ACFG_B') "resp='$($r.resp)'"

# -gu in config file
Write-Test "Config -gu via source-file"
"set -g @cfg-gu CFGVAL`nset -gu @cfg-gu" | Set-Content $cfgFile -Encoding UTF8
Send-Tcp "source-file `"$cfgFile`"" | Out-Null
Start-Sleep -Milliseconds 500
$r = Send-Tcp "show-options -gqv @cfg-gu"
Test-Result "cfg-combined-gu" ($r.ok -and $r.resp -ne 'CFGVAL') "resp='$($r.resp)'"

# -go (only if unset) in config file
Write-Test "Config -go via source-file (existing value preserved)"
"set -g escape-time 42`nset -go escape-time 999" | Set-Content $cfgFile -Encoding UTF8
Send-Tcp "source-file `"$cfgFile`"" | Out-Null
Start-Sleep -Milliseconds 500
$r = Send-Tcp "show-options -gqv escape-time"
Test-Result "cfg-combined-go" ($r.ok -and $r.resp -eq '42') "resp='$($r.resp)'"

# Restore escape-time
Send-Tcp "set-option -g escape-time 500" | Out-Null

# ---------------------------------------------------------------
# Cleanup
# ---------------------------------------------------------------
Remove-Item $cfgFile -ErrorAction SilentlyContinue
& $psmux kill-session -t $SESSION 2>&1 | Out-Null

Write-Host "`n========================================" -ForegroundColor White
Write-Host "Combined Flag Test Results:" -ForegroundColor White
Write-Host "  Passed: $pass" -ForegroundColor Green
Write-Host "  Failed: $fail" -ForegroundColor $(if($fail -gt 0){'Red'}else{'Green'})
Write-Host "  Total:  $total" -ForegroundColor White
Write-Host "========================================" -ForegroundColor White

if ($fail -gt 0) { exit 1 }
