# Issue #729: an Ink based TUI (dsh-TUI) in a psmux pane read the Up arrow as
# a lone Escape followed by the text "[A".
#
# Measured before the fix with a recorder pane (tests\key_chunk_probe729.cs,
# ReadFile in raw VT input mode, one log line per read):
#
#   pane did not ask for win32 input mode:  send-keys Up -> one read 1b5b41
#   pane asked for it (CSI ?9001h, as dsh-TUI does on Windows):
#       send-keys Up -> ESC[0;0;27;1;0;1_ ESC[0;0;91;1;0;1_ ESC[0;0;65;1;0;1_
#
# psmux wrote ESC [ A in ONE write either way (and an attached client in a real
# Windows Terminal window was the same, 0 splits in 50 presses).  The split was
# the pane's conhost: it hands a VT input reader the bytes psmux writes as one
# character record each, so a reader that also asked for win32 input mode got
# three keys, Escape, '[' and 'A'.  Windows Terminal sends every key as a win32
# input mode record (conhost asks it to), so the same child reads one VK_UP
# record there: ESC[38;72;0;1;256;1_ ESC[38;72;0;0;256;1_.  psmux now does the
# same for named keys on a pane that reads VT input.
#
# Part 1  send-keys into a win32 input mode pane: one VK record per key.
# Part 2  send-keys into a plain VT pane: the exact bytes tmux writes
#         (input-keys.c input_key_defaults), one read per key, Escape intact.
# Part 3  DECCKM pane: SS3 cursor keys, as tmux's MODE_KCURSOR.
# Part 4  a real attached client under a pseudoconsole (tests\conpty697.cs)
#         typing Up into a win32 input mode pane: split count must be 0.
#
# Windows 10 (a build before 22000, the inbox host): that conhost does not
# parse win32 input mode, so a pane that asked for it cannot get a record of
# the key from psmux; psmux writes tmux's bytes there (Issue #1097), and the
# pane reads them as one read each, as a pane that did not ask does.
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue729_key_chunks.ps1
$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = if ($env:PSMUX_TEST_NS) { $env:PSMUX_TEST_NS } else { "i729keys$PID" }
$script:Pass = 0; $script:Fail = 0; $script:Skip = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:Skip++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkGray }

foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE') { Remove-Item "Env:\$v" -EA SilentlyContinue }
$savedDataDir = $env:PSMUX_DATA_DIR
$savedNoWarm  = $env:PSMUX_NO_WARM
$root = Join-Path $env:TEMP "psmux_i729_$PID"
Remove-Item -Recurse -Force $root -EA SilentlyContinue
New-Item -ItemType Directory -Force $root | Out-Null
$env:PSMUX_DATA_DIR = Join-Path $root "data"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$env:PSMUX_NO_WARM = "1"
$emptyConf = Join-Path $root "empty.conf"
"" | Set-Content -Path $emptyConf -Encoding ASCII

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$probe = Join-Path $root "key_chunk_probe729.exe"
$conpty = Join-Path $root "conpty697.exe"
& $csc /nologo /optimize /platform:x64 /out:$probe (Join-Path $PSScriptRoot "key_chunk_probe729.cs") 2>&1 | Out-Null
& $csc /nologo /optimize /platform:x64 /out:$conpty (Join-Path $PSScriptRoot "conpty697.cs") 2>&1 | Out-Null
foreach ($exe in @($probe, $conpty)) {
    if (-not (Test-Path $exe)) { Write-Host "FATAL: could not compile $exe" -ForegroundColor Red; exit 1 }
}
Write-Info ("psmux: {0}  ({1})" -f $PSMUX, ((& $PSMUX -V) -join ' '))

$ESC = [char]27
# Issue #1097: the Windows 10 inbox host, which never parses win32 input mode.
$Win10 = [Environment]::OSVersion.Version.Build -lt 22000 -and -not $env:PSMUX_CONPTY_DIR
function Show([string]$s) { return $s.Replace([string]$ESC, '^[') }

# One string per ReadFile the probe logged.
function Read-Chunks([string]$log) {
    $out = @()
    foreach ($l in @(Get-Content $log -EA SilentlyContinue)) {
        if ($l -notmatch '^CHUNK t=\S+ n=\d+ ([0-9a-f]+)$') { continue }
        $h = $Matches[1]
        $sb = New-Object System.Text.StringBuilder
        for ($i = 0; $i -lt $h.Length; $i += 2) { [void]$sb.Append([char][Convert]::ToByte($h.Substring($i, 2), 16)) }
        $out += $sb.ToString()
    }
    return ,$out
}

# Win32 input mode records in a string: @{Vk; Sc; Uc; Kd; Cs}
function Parse-Records([string]$s) {
    $r = @()
    foreach ($m in [regex]::Matches($s, "\x1b\[(\d+);(\d+);(\d+);(\d+);(\d+);(\d+)_")) {
        $r += [pscustomobject]@{ Vk = [int]$m.Groups[1].Value; Sc = [int]$m.Groups[2].Value; Uc = [int]$m.Groups[3].Value; Kd = [int]$m.Groups[4].Value; Cs = [int]$m.Groups[5].Value }
    }
    return ,$r
}

function Start-Probe([string]$Sess, [string]$Opts) {
    $log = Join-Path $root "$Sess.log"; $stop = "$log.stop"
    Remove-Item $log, $stop -EA SilentlyContinue
    & $PSMUX -L $NS -f $emptyConf new-session -d -s $Sess -x 120 -y 30 -- $probe $log $stop $(if ($Opts) { $Opts } else { "-" }) 2>&1 | Out-Null
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 15000) {
        if (((& $PSMUX -L $NS capture-pane -p -t $Sess) -join "`n") -match 'key_chunk_probe729 ready') { break }
        Start-Sleep -Milliseconds 100
    }
    Start-Sleep -Milliseconds 400
    return @{ Log = $log; Stop = $stop; Sess = $Sess }
}
function Stop-Probe($p) {
    New-Item -ItemType File -Force $p.Stop | Out-Null
    & $PSMUX -L $NS kill-session -t $p.Sess 2>&1 | Out-Null
}

# Send one key with send-keys and return what the pane read for it.
function Send-One($p, [string[]]$Keys) {
    $before = (Read-Chunks $p.Log).Count
    & $PSMUX -L $NS send-keys -t $p.Sess @Keys 2>&1 | Out-Null
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $last = -1
    while ($sw.ElapsedMilliseconds -lt 3000) {
        $n = (Read-Chunks $p.Log).Count
        if ($n -gt $before -and $n -eq $last) { break }
        $last = $n
        Start-Sleep -Milliseconds 120
    }
    $all = Read-Chunks $p.Log
    if ($all.Count -le $before) { return ,@() }
    return ,@($all[$before..($all.Count - 1)])
}

try {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null

    # name, send-keys args, VK, control key state, tmux VT bytes, DECCKM bytes
    $keys = @(
        @('Up',      @('Up'),      38,  256, "$ESC[A",     "${ESC}OA"),
        @('Down',    @('Down'),    40,  256, "$ESC[B",     "${ESC}OB"),
        @('Left',    @('Left'),    37,  256, "$ESC[D",     "${ESC}OD"),
        @('Home',    @('Home'),    36,  256, "$ESC[H",     "${ESC}OH"),
        @('End',     @('End'),     35,  256, "$ESC[F",     "${ESC}OF"),
        @('PageUp',  @('PageUp'),  33,  256, "$ESC[5~",    "$ESC[5~"),
        @('DC',      @('DC'),      46,  256, "$ESC[3~",    "$ESC[3~"),
        @('F1',      @('F1'),      112, 0,   "${ESC}OP",   "${ESC}OP"),
        @('F5',      @('F5'),      116, 0,   "$ESC[15~",   "$ESC[15~"),
        @('F12',     @('F12'),     123, 0,   "$ESC[24~",   "$ESC[24~"),
        @('S-Up',    @('S-Up'),    38,  272, "$ESC[1;2A",  "$ESC[1;2A"),
        @('C-Right', @('C-Right'), 39,  264, "$ESC[1;5C",  "$ESC[1;5C"),
        @('M-Left',  @('M-Left'),  37,  258, "$ESC[1;3D",  "$ESC[1;3D"),
        @('BTab',    @('BTab'),    9,   16,  "$ESC[Z",     "$ESC[Z")
    )

    Write-Host "`n=== Part 1: send-keys into a pane that asked for win32 input mode (CSI ?9001h) ===" -ForegroundColor Yellow
    $p = Start-Probe "w32" "w32"
    foreach ($k in $keys) {
        $got = Send-One $p $k[1]
        $joined = $got -join ''
        if ($Win10) {
            if ($got.Count -eq 1 -and $got[0] -eq $k[4]) { Write-Pass ("{0}: one read {1} (this host has no win32 input mode)" -f $k[0], (Show $k[4])) }
            else { Write-Fail ("{0}: Windows 10: expected one read {1}, got {2} reads: {3}" -f $k[0], (Show $k[4]), $got.Count, (Show $joined)) }
            continue
        }
        $recs = Parse-Records $joined
        $charRecs = @($recs | Where-Object { $_.Vk -eq 0 })
        if ($recs.Count -eq 2 -and $charRecs.Count -eq 0 -and $recs[0].Vk -eq $k[2] -and $recs[0].Cs -eq $k[3] -and $recs[0].Kd -eq 1 -and $recs[1].Kd -eq 0) {
            Write-Pass ("{0}: one key record vk={1} cs={2}" -f $k[0], $k[2], $k[3])
        } else {
            Write-Fail ("{0}: expected one vk={1} cs={2} press and release, read {3} records ({4} character records): {5}" -f $k[0], $k[2], $k[3], $recs.Count, $charRecs.Count, (Show $joined))
        }
    }
    $got = (Send-One $p @('Escape')) -join ''
    $recs = Parse-Records $got
    if ($Win10) {
        if ($got -eq "$ESC") { Write-Pass "Escape arrives as a lone ESC (this host has no win32 input mode)" } else { Write-Fail "Windows 10 Escape: read $(Show $got)" }
    }
    elseif ($recs.Count -ge 1 -and $recs[0].Vk -eq 27) { Write-Pass "Escape arrives as the Escape key record" }
    else { Write-Fail "Escape: read $(Show $got)" }
    Stop-Probe $p

    Write-Host "`n=== Part 2: send-keys into a plain VT input pane: tmux's bytes, one read per key ===" -ForegroundColor Yellow
    $p = Start-Probe "vt" ""
    foreach ($k in $keys) {
        $got = Send-One $p $k[1]
        if ($got.Count -eq 1 -and $got[0] -eq $k[4]) { Write-Pass ("{0}: one read {1}" -f $k[0], (Show $k[4])) }
        else { Write-Fail ("{0}: expected one read {1}, got {2} reads: {3}" -f $k[0], (Show $k[4]), $got.Count, ((($got | ForEach-Object { Show $_ }) -join ' | '))) }
    }
    $got = (Send-One $p @('Escape')) -join ''
    $got2 = (Send-One $p @('-l', 'b')) -join ''
    if ($got -eq "$ESC" -and $got2 -eq 'b') { Write-Pass "a lone Escape still reaches the pane after the key records, and is not fused with the next key" }
    else { Write-Fail "Escape then b: read '$(Show $got)' then '$(Show $got2)'" }
    Stop-Probe $p

    Write-Host "`n=== Part 3: DECCKM pane gets SS3 cursor keys (tmux MODE_KCURSOR) ===" -ForegroundColor Yellow
    $p = Start-Probe "ckm" "ckm"
    foreach ($k in $keys | Where-Object { $_[0] -in 'Up', 'Down', 'Left', 'Home', 'End', 'S-Up', 'F1' }) {
        $got = Send-One $p $k[1]
        if ($got.Count -eq 1 -and $got[0] -eq $k[5]) { Write-Pass ("{0}: one read {1}" -f $k[0], (Show $k[5])) }
        else { Write-Fail ("{0}: expected {1}, got {2}" -f $k[0], (Show $k[5]), ((($got | ForEach-Object { Show $_ }) -join ' | '))) }
    }
    Stop-Probe $p

    Write-Host "`n=== Part 4: an attached client typing Up into a win32 input mode pane ===" -ForegroundColor Yellow
    $p = Start-Probe "att" "w32"
    $script = Join-Path $root "att_script.txt"
    $lines = @("WAIT 3000")
    for ($i = 0; $i -lt 10; $i++) { $lines += "HEX 1b 5b 41"; $lines += "WAIT 150" }
    $lines += "WAIT 800"; $lines += "END"
    $lines | Set-Content $script -Encoding ASCII
    $before = (Read-Chunks $p.Log).Count
    $cp = Start-Process -FilePath $conpty -ArgumentList $script, (Join-Path $root "att.out"), 120, 30, 0, "`"$PSMUX`" -L $NS attach -t att" -PassThru -WindowStyle Hidden
    if (-not $cp.WaitForExit(60000)) { Stop-Process -Id $cp.Id -Force -EA SilentlyContinue }
    Start-Sleep -Milliseconds 300
    $all = Read-Chunks $p.Log
    $joined = if ($all.Count -gt $before) { ($all[$before..($all.Count - 1)]) -join '' } else { '' }
    $recs = Parse-Records $joined
    $ups = @($recs | Where-Object { $_.Vk -eq 38 -and $_.Kd -eq 1 }).Count
    $splits = @($recs | Where-Object { $_.Vk -eq 0 -and $_.Uc -eq 27 }).Count
    Write-Info "10 Up presses: $ups VK_UP presses, $splits lone ESC character records"
    if ($ups -eq 10 -and $splits -eq 0) { Write-Pass "10 of 10 Up presses arrived as one VK_UP record, 0 split" }
    elseif ($ups -eq 0 -and $splits -eq 0) { Write-Skip "the attached client delivered nothing (pseudoconsole host refused?): $(Show $joined)" }
    else { Write-Fail "Up from an attached client: $ups whole, $splits split into ESC [ A" }
    Stop-Probe $p
}
finally {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    $env:PSMUX_DATA_DIR = $savedDataDir
    $env:PSMUX_NO_WARM = $savedNoWarm
    Remove-Item -Recurse -Force $root -EA SilentlyContinue
}

Write-Host ""
Write-Host ("Results: {0} passed, {1} failed, {2} skipped" -f $script:Pass, $script:Fail, $script:Skip)
if ($script:Fail -gt 0) { exit 1 } else { exit 0 }
