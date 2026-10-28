# Issue #924: over SSH (Windows OpenSSH, session 0) with Store (MSIX) pwsh as
# the only PowerShell 7, new-session could not create any pane:
#   CreateProcessW ...\WindowsApps\pwsh.exe ... Access is denied. (os error 5)
#
# WHAT WAS REPRODUCED (9b5c3e0e, Windows 11 26200, ssh localhost, session 0)
#   SHELL unset and SSH_CONNECTION / SSH_CLIENT removed (the environment of a
#   service, a scheduled task, or an SSH command whose env was scrubbed):
#     new-session -d -s t                 -> rc 1, Access is denied (os error 5)
#     new-session -d -s t "ping ... | more" -> rc 1, same error
#   The old guard only looked at the SSH markers, so it never fired there.
#   (The reporter's rc 0 with no message was 3.3.8; master already prints the
#   spawn error and exits 1, which T4 pins.)
#
# tmux PARITY
#   spawn.c:411-413 keeps default-shell only when checkshell() (tmux.c:98) says
#   it is executable, else _PATH_BSHELL. Windows reading: a Store packaged shell
#   is not executable in session 0, so the default falls back to classic Windows
#   PowerShell, then cmd.exe. cmd-new-session.c:305-309 fails new-session with
#   "create window failed" when the first pane cannot spawn.
#
# TESTS
#   T1  ssh, SHELL and SSH markers removed, no command: session exists, pane is
#       classic powershell.exe (FAILS on master: rc 1, Access denied)
#   T2  same with a shell syntax command: session exists, command runs under
#       powershell.exe (FAILS on master)
#   T3  ssh, SHELL set to the Store alias: falls back to powershell.exe
#       (FAILS on master: the SHELL value was used verbatim)
#   T4  ssh, explicit default-shell = Store alias in a config: new-session
#       still fails loudly (rc != 0, spawn error on stderr), never silently
#   T5  local interactive session: the default is still the Store pwsh (the
#       fallback only applies where Store activation cannot work)
#   P1  new-session -d time, local and over ssh, written to metrics JSON
#
# Needs: Store pwsh on PATH and `ssh localhost` key auth (BatchMode). SKIPs
# otherwise. Fully isolated: private -L namespaces and PSMUX_DATA_DIR.
# Set PSMUX_TEST_BIN to test a non installed binary.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else {
    $local = Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue
    if ($local) { $local.Path } else { (Get-Command psmux -EA Stop).Source }
}
$script:Pass = 0; $script:Fail = 0; $script:Skip = 0
function Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:Skip++ }

$work = Join-Path ([IO.Path]::GetTempPath()) "psmux_t924_$PID"
New-Item -ItemType Directory -Force $work | Out-Null
$alias = Join-Path $env:LOCALAPPDATA "Microsoft\WindowsApps\pwsh.exe"
$pw = Get-Command pwsh -EA SilentlyContinue
if (-not $pw -or $pw.Source -notmatch '\\WindowsApps\\' -or -not (Test-Path $alias)) {
    Skip "Store pwsh is not the pwsh on PATH; nothing to test"; exit 0
}

# Inner script run on the far side of ssh. Prints one JSON line.
$inner = Join-Path $work "inner.ps1"
@'
param([string]$Bin, [string]$Ns, [string]$DataDir, [string]$Mode, [string]$Cmd = "", [string]$Conf = "")
$env:PSMUX_DATA_DIR = $DataDir
New-Item -ItemType Directory -Force $DataDir | Out-Null
if ($Mode -eq 'scrub') { Remove-Item env:SHELL, env:SSH_CONNECTION, env:SSH_CLIENT, env:SSH_TTY -EA SilentlyContinue }
if ($Mode -eq 'storeshell') { $env:SHELL = Join-Path $env:LOCALAPPDATA "Microsoft\WindowsApps\pwsh.exe" }
$a = @('-L', $Ns)
if ($Conf) { $a += @('-f', $Conf) }
$a += @('new-session', '-d', '-s', 't')
if ($Cmd) { $a += $Cmd }
$sw = [Diagnostics.Stopwatch]::StartNew()
$out = & $Bin @a 2>&1
$rc = $LASTEXITCODE; $ms = $sw.ElapsedMilliseconds
Start-Sleep -Milliseconds 1500
$has = $false; $names = @()
& $Bin -L $Ns has-session -t t 2>$null; if ($LASTEXITCODE -eq 0) { $has = $true }
if ($has) {
  $pp = (& $Bin -L $Ns display -p -t t '#{pane_pid}' 2>$null) -as [int]
  if ($pp) { $names = @(Get-CimInstance Win32_Process -Filter "ProcessId=$pp OR ParentProcessId=$pp" | % { $_.Name }) }
}
& $Bin -L $Ns kill-server 2>&1 | Out-Null
[pscustomobject]@{ session = [Diagnostics.Process]::GetCurrentProcess().SessionId; rc = $rc; ms = $ms
  out = ($out -join ' | '); has = $has; procs = ($names -join ',') } | ConvertTo-Json -Compress
'@ | Set-Content -Encoding UTF8 $inner

$probe = ssh -o BatchMode=yes -o ConnectTimeout=5 localhost "echo ok" 2>$null
if ($probe -ne 'ok') { Skip "ssh localhost key auth unavailable; session 0 cases not runnable" }

function Remote($ns, $mode, $cmd = "", $conf = "") {
    $dd = Join-Path $work "dd_$ns"
    $line = "powershell -NoProfile -ExecutionPolicy Bypass -File `"$inner`" -Bin `"$PSMUX`" -Ns $ns -DataDir `"$dd`" -Mode $mode"
    if ($cmd) { $line += " -Cmd '$cmd'" }
    if ($conf) { $line += " -Conf `"$conf`"" }
    $raw = ssh -o BatchMode=yes localhost $line 2>&1
    $json = $raw | Where-Object { "$_".StartsWith('{') } | Select-Object -Last 1
    if (-not $json) { return [pscustomobject]@{ rc = -1; out = ($raw -join ' '); has = $false; procs = ''; session = -1; ms = -1 } }
    $json | ConvertFrom-Json
}

$metrics = [ordered]@{ issue = 924; bin = $PSMUX; time = (Get-Date).ToString('o') }
$tag = Get-Random -Maximum 99999

if ($probe -eq 'ok') {
    Write-Host "T1: session 0, SHELL and SSH markers removed, default shell"
    $r = Remote "t924a$tag" 'scrub'
    if ($r.session -eq 0 -and $r.rc -eq 0 -and $r.has -and $r.procs -match 'powershell\.exe' -and $r.procs -notmatch 'pwsh') {
        Pass "session 0, rc 0, pane runs [$($r.procs)]"
    } else { Fail "session=$($r.session) rc=$($r.rc) has=$($r.has) procs=[$($r.procs)] out=[$($r.out.Substring(0, [Math]::Min(200, $r.out.Length)))]" }
    $metrics.ssh_scrub_new_session_ms = $r.ms

    Write-Host "T2: same, with a shell syntax command"
    $r = Remote "t924b$tag" 'scrub' 'ping -n 30 127.0.0.1 | more'
    if ($r.rc -eq 0 -and $r.has -and $r.procs -match 'powershell\.exe' -and $r.procs -match 'PING') {
        Pass "command runs under [$($r.procs)]"
    } else { Fail "rc=$($r.rc) has=$($r.has) procs=[$($r.procs)] out=[$($r.out.Substring(0, [Math]::Min(200, $r.out.Length)))]" }

    Write-Host "T3: SHELL = Store alias over ssh"
    $r = Remote "t924c$tag" 'storeshell'
    if ($r.rc -eq 0 -and $r.has -and $r.procs -match 'powershell\.exe' -and $r.procs -notmatch 'pwsh') {
        Pass "Store SHELL skipped, pane runs [$($r.procs)]"
    } else { Fail "rc=$($r.rc) has=$($r.has) procs=[$($r.procs)] out=[$($r.out.Substring(0, [Math]::Min(200, $r.out.Length)))]" }

    Write-Host "T4: explicit default-shell = Store alias fails loudly"
    $conf = Join-Path $work "store.conf"
    "set -g default-shell `"$alias`"" | Set-Content $conf
    $r = Remote "t924d$tag" 'keep' '' $conf
    if ($r.rc -ne 0 -and -not $r.has -and $r.out -match 'spawn shell error' -and $r.out -match 'os error 5') {
        Pass "rc=$($r.rc), stderr names the spawn error"
    } else { Fail "rc=$($r.rc) has=$($r.has) out=[$($r.out.Substring(0, [Math]::Min(200, $r.out.Length)))]" }

    $times = @(); foreach ($i in 1..5) { $times += (Remote "t924p$tag$i" 'keep').ms }
    $metrics.ssh_new_session_ms = $times
}

Write-Host "T5: local interactive session keeps Store pwsh as the default"
$ns = "t924e$tag"; $env:PSMUX_DATA_DIR = Join-Path $work "dd_$ns"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$saved = $env:SHELL; Remove-Item env:SHELL -EA SilentlyContinue
& $PSMUX -L $ns new-session -d -s t 2>&1 | Out-Null
$ds = & $PSMUX -L $ns show-options -g -v default-shell 2>&1
& $PSMUX -L $ns kill-server 2>&1 | Out-Null
if ($ds -match '\\WindowsApps\\(.*\\)?pwsh\.exe$') { Pass "default-shell = $ds" } else { Fail "default-shell = [$ds]" }

$times = @()
foreach ($i in 1..5) {
    $ns = "t924q$tag$i"; $env:PSMUX_DATA_DIR = Join-Path $work "dd_$ns"
    New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
    $sw = [Diagnostics.Stopwatch]::StartNew(); & $PSMUX -L $ns new-session -d -s t 2>&1 | Out-Null; $times += $sw.ElapsedMilliseconds
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
}
if ($null -ne $saved) { $env:SHELL = $saved }
$metrics.local_new_session_ms = $times
$mdir = Join-Path $env:USERPROFILE ".psmux-test-data\metrics"
New-Item -ItemType Directory -Force $mdir | Out-Null
$metrics | ConvertTo-Json | Set-Content (Join-Path $mdir "issue924_store_shell_session0.json")
Write-Host "  P1: local new-session ms = $($times -join ',')"
if ($metrics.ssh_new_session_ms) { Write-Host "  P1: ssh new-session ms = $($metrics.ssh_new_session_ms -join ',')" }

Write-Host "`nResult: $script:Pass passed, $script:Fail failed, $script:Skip skipped"
Remove-Item -Recurse -Force $work -EA SilentlyContinue
exit $(if ($script:Fail -gt 0) { 1 } else { 0 })
