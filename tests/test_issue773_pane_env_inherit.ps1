# Issue #773: a pane inherits the environment of the shell that started the
# server, it is no longer rebuilt from the registry.
#
# BEFORE (master 10640ec8, 3 of 3 runs, the reporter's probe):
#   MARK=[inherited] TEMP=[C:\Users\<user>\AppData\Local\Temp] OS=[Windows_NT] pathmarker=False
# The launcher had TEMP=C:\temp-marker, OS=os-marker and C:\path-marker first on
# PATH. crates/portable-pty-psmux/src/cmdbuilder.rs get_base_env() laid
# HKLM Session Manager\Environment and HKCU\Environment OVER the inherited
# environment (wezterm's portable-pty behaviour), so every key the registry also
# defines took the registry value in every pane.
#
# tmux: the server's start environment is passed through unchanged
# (tmux.c fills global_environ from environ; spawn.c builds the child from it).
# The registry now only fills keys the process environment lacks.
#
# WHAT THIS SUITE PINS (all with an isolated PSMUX_DATA_DIR and -L namespace)
#   CLI   cold new-session, new-window (pool spare), split-window, respawn-pane
#   TCP   new-window sent over a raw authenticated connection
#   WARM  a standby parked by a shell WITHOUT the markers, claimed by one WITH
#         them: window 0 (respawned because the claim changed PATH), a window
#         created after the claim, and the claiming cwd is still honoured
#   WARM  a second claim from the same environment does NOT respawn window 0
#   FILL  a launcher without OS still gets the registry OS (fill, not drop)
#   PATH  no launcher entry dropped, nothing duplicated
#   TUI   an attached client in a visible window: its pane sees the launcher's
#         environment
#
# Set PSMUX_TEST_BIN to test a non-installed binary.
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue773_pane_env_inherit.ps1

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else {
    $local = Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue
    if ($local) { $local.Path } else { (Get-Command psmux -EA Stop).Source }
}
$script:Pass = 0; $script:Fail = 0; $script:Skip = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:Skip++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }
function Write-Head($m) { Write-Host "`n$m" -ForegroundColor Cyan }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX_TARGET_SESSION') {
    Set-Item -Path "env:$v" -Value $null -EA SilentlyContinue
}

$tag8 = [guid]::NewGuid().ToString('N').Substring(0, 6)
$NS   = "t773_$tag8"
$NSF  = "t773f_$tag8"
$rig  = Join-Path $env:TEMP "psmux773-$tag8"
$DATA = Join-Path $rig 'data'
$report = Join-Path $rig 'report.txt'
$probe  = Join-Path $rig 'probe.ps1'
$seedCwd   = Join-Path $rig 'seed'
$clientCwd = Join-Path $rig 'client'
$PATH_MARK = Join-Path $rig 'path-marker'
$TEMP_MARK = Join-Path $rig 'temp-marker'
New-Item -ItemType Directory -Force -Path $rig, $DATA, $seedCwd, $clientCwd, $PATH_MARK, $TEMP_MARK | Out-Null
$env:PSMUX_DATA_DIR = $DATA
$script:Opened = @()

@'
param([string]$Tag, [string]$Out)
$lines = @(
    "tag=$Tag",
    "temp=$env:TEMP",
    "os=$env:OS",
    "mark=$env:BH_MARK773",
    "path=$env:PATH",
    "end=1"
)
Add-Content -LiteralPath $Out -Value $lines
'@ | Set-Content -LiteralPath $probe -Encoding UTF8

function Read-Report([string]$Tag) {
    if (-not (Test-Path $report)) { return $null }
    $cur = @{}; $out = $null
    foreach ($line in Get-Content -LiteralPath $report) {
        $kv = $line -split '=', 2
        if ($kv.Count -ne 2) { continue }
        if ($kv[0] -eq 'tag') { $cur = @{ tag = $kv[1] } } else { $cur[$kv[0]] = $kv[1] }
        if ($cur['tag'] -eq $Tag -and $cur.ContainsKey('end')) { $out = $cur.Clone() }
    }
    return $out
}
function Wait-Report([string]$Tag, [int]$Sec = 30) {
    $deadline = [DateTime]::Now.AddSeconds($Sec)
    while ([DateTime]::Now -lt $deadline) {
        $r = Read-Report $Tag
        if ($r) { return $r }
        Start-Sleep -Milliseconds 250
    }
    return $null
}
function Norm([string]$p) { return $p.TrimEnd([char[]]("\", "/")) }
function Path-Entries([string]$p) { return @($p -split ';' | Where-Object { $_ } | ForEach-Object { Norm $_ }) }
$probeCmd = { param($t) "pwsh -NoProfile -ExecutionPolicy Bypass -File `"$probe`" $t `"$report`"" }

# The launcher environment every pane is expected to inherit.
$env:PATH = "$PATH_MARK;" + $env:PATH
$env:TEMP = $TEMP_MARK
$env:OS = 'os-marker-773'
$env:BH_MARK773 = 'inherited'
$launcherPath = Path-Entries $env:PATH

function Check-Inherited([string]$What, $r) {
    if (-not $r) { Write-Fail "$What never reported back"; return }
    $entries = Path-Entries $r['path']
    $markCount = @($entries | Where-Object { $_ -ieq (Norm $PATH_MARK) }).Count
    $ok = ($r['temp'] -eq $TEMP_MARK) -and ($r['os'] -eq 'os-marker-773') -and ($r['mark'] -eq 'inherited') -and ($markCount -eq 1)
    if ($ok) {
        Write-Pass "$What sees the launcher's TEMP, OS, PATH entry and control variable"
    } else {
        Write-Fail ("{0}: TEMP=[{1}] OS=[{2}] MARK=[{3}] path-marker entries={4}" -f $What, $r['temp'], $r['os'], $r['mark'], $markCount)
    }
    # Nothing dropped: every launcher entry is still there. Nothing invented:
    # the only entries the launcher did not have are the ones pwsh prepends
    # for itself at startup (its own install directory).
    $missing = @($launcherPath | Where-Object { $e = $_; -not ($entries | Where-Object { $_ -ieq $e }) })
    $pwshDir = Norm (Split-Path (Get-Command pwsh).Source -Parent)
    $extra = @($entries | Where-Object { $e = $_; -not ($launcherPath | Where-Object { $_ -ieq $e }) -and $e -ine $pwshDir })
    if ($missing.Count -eq 0 -and $extra.Count -eq 0) {
        Write-Pass "$What PATH keeps every launcher entry and adds nothing from the registry"
    } else {
        Write-Fail "$What PATH missing=[$($missing -join ';')] extra=[$($extra -join ';')]"
    }
}

function Cleanup {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    & $PSMUX -L $NSF kill-server 2>&1 | Out-Null
    foreach ($id in $script:Opened) { try { Stop-Process -Id $id -Force -EA SilentlyContinue } catch {} }
    Start-Sleep -Milliseconds 700
}

function Wait-Standby([string]$Ns, [int]$TimeoutMs = 15000) {
    $port = Join-Path $DATA "$($Ns)____warm__.port"
    $pidf = Join-Path $DATA "$($Ns)____warm__.pid"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        if ((Test-Path $port) -and (Test-Path $pidf)) {
            $raw = Get-Content $pidf -Raw -EA SilentlyContinue
            if ($raw) {
                $id = ($raw.Trim() -split ':')[0]
                if ($id -and (Get-Process -Id ([int]$id) -EA SilentlyContinue)) { return $id }
            }
        }
        Start-Sleep -Milliseconds 150
    }
    return ''
}

function Invoke-InShell([string]$Body) {
    $f = Join-Path $rig ("shell-" + [guid]::NewGuid().ToString('N').Substring(0, 6) + ".ps1")
    Set-Content -LiteralPath $f -Value $Body -Encoding UTF8
    pwsh -NoProfile -ExecutionPolicy Bypass -File $f | Out-Null
}

function Send-Tcp([string]$Ns, [string]$Session, [string]$Cmd) {
    $base = "$($Ns)__$Session"
    $port = [int](Get-Content (Join-Path $DATA "$base.port") -Raw).Trim()
    $key = (Get-Content (Join-Path $DATA "$base.key") -Raw).Trim()
    $tcp = New-Object System.Net.Sockets.TcpClient
    $tcp.NoDelay = $true
    $tcp.Connect("127.0.0.1", $port)
    $st = $tcp.GetStream(); $st.ReadTimeout = 15000
    $wr = New-Object System.IO.StreamWriter($st); $rd = New-Object System.IO.StreamReader($st)
    $wr.WriteLine("AUTH $key"); $wr.Flush()
    $auth = $rd.ReadLine()
    $wr.WriteLine($Cmd); $wr.Flush()
    $out = @()
    try { while ($true) { $l = $rd.ReadLine(); if ($null -eq $l) { break }; $out += $l } } catch {}
    $tcp.Close()
    return @{ auth = $auth; out = $out }
}

function Pane-StartedAfter([string]$Target, [datetime]$T) {
    $ppid_ = (& $PSMUX -L $NS display-message -t $Target -p '#{pane_pid}' 2>&1 | Out-String).Trim()
    if ($ppid_ -notmatch '^\d+$') { return $null }
    $p = Get-Process -Id ([int]$ppid_) -EA SilentlyContinue
    if (-not $p) { return $null }
    return ($p.StartTime -gt $T)
}

try {

# ── 1. WARM: park a standby from a shell WITHOUT the markers ─────────────────
Write-Head "1. warm standby parked by a shell without the markers"
$seedPath = (@($env:PATH -split ';' | Where-Object { (Norm $_) -ine (Norm $PATH_MARK) }) -join ';')
Invoke-InShell @"
Set-Location '$seedCwd'
`$env:PATH = '$seedPath'
`$env:TEMP = '$([IO.Path]::GetTempPath().TrimEnd('\'))'
`$env:OS = 'Windows_NT'
`$env:BH_MARK773 = `$null
& '$PSMUX' -L $NS new-session -d -s seed
"@
$warmPid = Wait-Standby $NS
if ($warmPid) { Write-Pass "standby parked (pid $warmPid)" } else { Write-Skip "no standby parked; warm cases below cannot exercise the claim" }
& $PSMUX -L $NS kill-session -t seed 2>&1 | Out-Null
$w2 = Wait-Standby $NS; if ($w2) { $warmPid = $w2 }

# ── 2. WARM: claim it from THIS process (markers set) ────────────────────────
Write-Head "2. claim from the launcher with the markers"
Push-Location $clientCwd
$claimStart = Get-Date
$sw = [Diagnostics.Stopwatch]::StartNew()
& $PSMUX -L $NS new-session -d -s claimed
$claimMs = $sw.ElapsedMilliseconds
Pop-Location
$sessPid = (& $PSMUX -L $NS display-message -t claimed -p '#{pid}' 2>&1 | Out-String).Trim()
$claimed = $warmPid -and ($sessPid -eq $warmPid)
if ($claimed) { Write-Pass "new-session claimed the standby (pid $sessPid, $claimMs ms)" } else { Write-Skip "new-session cold spawned (pid '$sessPid' vs standby '$warmPid')" }
Start-Sleep -Milliseconds 2500
if ($claimed) {
    $re = Pane-StartedAfter 'claimed:0.0' $claimStart
    if ($re -eq $true) { Write-Pass "window 0 was respawned because the claim changed PATH" } else { Write-Fail "window 0 still runs the standby's shell (respawned=$re)" }
}
& $PSMUX -L $NS send-keys -t claimed:0.0 (& $probeCmd 'WARM0') Enter 2>&1 | Out-Null
Check-Inherited "claimed window 0" (Wait-Report 'WARM0')
$pcp = (& $PSMUX -L $NS display-message -t claimed:0.0 -p '#{pane_current_path}' 2>&1 | Out-String).Trim()
if ((Norm $pcp) -ieq (Norm $clientCwd)) { Write-Pass "claimed window 0 starts in the claiming cwd" } else { Write-Fail "claimed window 0 cwd '$pcp', expected '$clientCwd'" }

# ── 3. CLI: new-window / split-window / respawn-pane in the claimed session ──
Write-Head "3. CLI spawns in the claimed session"
& $PSMUX -L $NS new-window -t claimed (& $probeCmd 'NEWWIN') 2>&1 | Out-Null
Check-Inherited "new-window" (Wait-Report 'NEWWIN')
# A window running the probe closes when the probe exits, so later targets
# go by pane id, never by window index.
$spare = (& $PSMUX -L $NS new-window -t claimed -P -F '#{pane_id}' 2>&1 | Out-String).Trim()
Start-Sleep -Milliseconds 1500
& $PSMUX -L $NS send-keys -t $spare (& $probeCmd 'SPAREWIN') Enter 2>&1 | Out-Null
Check-Inherited "new-window shell (pool spare)" (Wait-Report 'SPAREWIN')
& $PSMUX -L $NS split-window -t $spare (& $probeCmd 'SPLIT') 2>&1 | Out-Null
Check-Inherited "split-window" (Wait-Report 'SPLIT')
& $PSMUX -L $NS respawn-pane -k -t $spare (& $probeCmd 'RESPAWN') 2>&1 | Out-Null
Check-Inherited "respawn-pane -k" (Wait-Report 'RESPAWN')

# ── 4. TCP: new-window over a raw authenticated connection ───────────────────
# The wire takes the command as ONE argument (the CLI quotes it the same way).
Write-Head "4. TCP new-window"
$t = Send-Tcp $NS 'claimed' ("new-window '" + (& $probeCmd 'TCPWIN') + "'")
if ($t.auth -eq 'OK') { Write-Pass "TCP AUTH accepted" } else { Write-Fail "TCP AUTH reply '$($t.auth)'" }
Check-Inherited "TCP new-window" (Wait-Report 'TCPWIN')

# ── 5. WARM: a claim from the SAME environment leaves window 0 alone ─────────
Write-Head "5. same environment claim keeps the pre-spawned window 0"
$w3 = Wait-Standby $NS
if ($w3) {
    Start-Sleep -Milliseconds 1500
    Push-Location $clientCwd
    $claim2Start = Get-Date
    $sw = [Diagnostics.Stopwatch]::StartNew()
    & $PSMUX -L $NS new-session -d -s claimed2
    $claim2Ms = $sw.ElapsedMilliseconds
    Pop-Location
    $p2 = (& $PSMUX -L $NS display-message -t claimed2 -p '#{pid}' 2>&1 | Out-String).Trim()
    if ($p2 -eq $w3) {
        Write-Info "second claim took $claim2Ms ms (first, PATH changing, took $claimMs ms)"
        $re2 = Pane-StartedAfter 'claimed2:0.0' $claim2Start
        if ($re2 -eq $false) { Write-Pass "window 0 kept its pre-spawned shell (no respawn when PATH is unchanged)" } else { Write-Fail "window 0 was respawned on a claim that did not change PATH (respawned=$re2)" }
        & $PSMUX -L $NS send-keys -t claimed2:0.0 (& $probeCmd 'WARM2') Enter 2>&1 | Out-Null
        Check-Inherited "second claim window 0" (Wait-Report 'WARM2')
    } else { Write-Skip "second new-session did not claim the standby" }
} else { Write-Skip "no replacement standby was parked" }

# ── 6. FILL: a launcher WITHOUT OS still gets the registry value ─────────────
Write-Head "6. registry fills a key the launcher lacks"
Invoke-InShell @"
`$env:OS = `$null
& '$PSMUX' -L $NSF new-session -d -s fill pwsh -NoProfile -ExecutionPolicy Bypass -File "$probe" FILL "$report"
"@
$r = Wait-Report 'FILL'
$regOs = (Get-ItemProperty 'HKLM:\System\CurrentControlSet\Control\Session Manager\Environment' -Name OS -EA SilentlyContinue).OS
if (-not $r) { Write-Fail "fill pane never reported back" }
elseif ($regOs -and $r['os'] -eq $regOs) { Write-Pass "OS missing from the launcher is filled from the registry ($regOs)" }
else { Write-Fail "fill pane OS=[$($r['os'])], registry OS=[$regOs]" }
if ($r -and $r['temp'] -eq $TEMP_MARK) { Write-Pass "fill case still keeps the launcher's TEMP" } elseif ($r) { Write-Fail "fill case TEMP=[$($r['temp'])]" }

# ── 7. TUI: an attached client in a visible window ───────────────────────────
Write-Head "7. attached client (visible window)"
$cli = Start-Process -FilePath $PSMUX -ArgumentList "-L", $NS, "new-session", "-s", "tui", "-x", "110", "-y", "30" -PassThru
$script:Opened += $cli.Id
$up = $false
for ($i = 0; $i -lt 60; $i++) { Start-Sleep -Milliseconds 250; & $PSMUX -L $NS has-session -t tui 2>$null; if ($LASTEXITCODE -eq 0) { $up = $true; break } }
if (-not $up) { Write-Fail "attached session never came up" }
else {
    Start-Sleep -Milliseconds 2500
    & $PSMUX -L $NS send-keys -t tui (& $probeCmd 'TUI') Enter 2>&1 | Out-Null
    Check-Inherited "attached client's pane" (Wait-Report 'TUI')
    & $PSMUX -L $NS send-keys -t tui 'echo "SEEN=$env:OS"' Enter 2>&1 | Out-Null
    $seen = $false
    for ($i = 0; $i -lt 20; $i++) {
        Start-Sleep -Milliseconds 250
        $cap = (& $PSMUX -L $NS capture-pane -p -t tui 2>&1 | Out-String)
        if ($cap -match '(?m)^SEEN=os-marker-773') { $seen = $true; break }
    }
    if ($seen) { Write-Pass "the attached pane's screen shows the launcher's OS value" } else { Write-Fail "the attached pane's screen never showed SEEN=os-marker-773" }
    if (-not $cli.HasExited) { Write-Pass "attached client still running" } else { Write-Fail "attached client exited" }
}

}
finally {
    Cleanup
    Remove-Item -LiteralPath $rig -Recurse -Force -EA SilentlyContinue
}

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed:  $($script:Pass)" -ForegroundColor Green
Write-Host "  Failed:  $($script:Fail)" -ForegroundColor $(if ($script:Fail -gt 0) { "Red" } else { "Green" })
Write-Host "  Skipped: $($script:Skip)" -ForegroundColor Yellow
exit $(if ($script:Fail -gt 0) { 1 } else { 0 })
