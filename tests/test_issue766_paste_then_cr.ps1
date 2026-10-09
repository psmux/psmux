# Issue #766: a CR written 150 to 300 ms after a bracketed paste reached the
# pane INSIDE the paste, before ESC[201~.
#
# The reporter drives a psmux client through ConPTY (node-pty, the way a web
# terminal front end does), writes ESC[200~ <~500 chars> ESC[201~ and then a
# lone CR. The console host strips the paste markers on the client's console
# input route and hands over key records identical to typed ones, so the client
# held the text in stage 2 for its 300 ms window and appended the CR to it: the
# pane (DECSET 2004 on) read ESC[200~ text CR ESC[201~, a pasted newline
# instead of Enter. Also seen: the first character, when it needs Shift, went
# out alone in front of ESC[200~, and an em dash (handed over as an Alt code)
# never arrived at all.
#
# This suite reproduces it with a C# pseudoconsole host (conpty_paste766.cs)
# instead of node-pty, so it needs nothing but csc:
#   1. console input route (the default): a CR written 200 and 250 ms after the
#      paste arrives AFTER ESC[201~ (before the fix it arrived inside; master
#      held the paste until about +310 ms), and the em dash survives.
#   2. PSMUX_VT_INPUT=1 (the VT input route, where conhost passes the markers
#      through): the paste is forwarded exactly as written, first byte ESC,
#      content byte exact, and a CR only 50 ms behind it still lands after
#      ESC[201~, which is what tmux does (tty-keys.c tty_keys_paste).
#
# Runs in its own -L namespace per case and cleans up only that namespace.
# PSMUX_TEST_BIN selects the binary under test.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$Reps = if ($env:PSMUX_TEST_REPS) { [int]$env:PSMUX_TEST_REPS } else { 3 }
$script:Pass = 0; $script:Fail = 0; $script:Skip = 0

function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:Skip++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkGray }

Write-Host "=== Issue #766: a CR written after a bracketed paste stays after ESC[201~ ===" -ForegroundColor Cyan
Write-Info "binary: $PSMUX"

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
}
if (-not (Test-Path $csc)) { Write-Skip "csc.exe not found"; exit 0 }

$root = Join-Path $env:TEMP ("psmux_i766_" + (Get-Random -Maximum 999999))
New-Item -ItemType Directory -Force $root | Out-Null
$hostExe = Join-Path $root "conpty_paste766.exe"
$childExe = Join-Path $root "paste766_child.exe"
& $csc /nologo /platform:x64 /out:$hostExe "$PSScriptRoot\conpty_paste766.cs" 2>&1 | Out-Null
& $csc /nologo /platform:x64 /out:$childExe "$PSScriptRoot\paste766_child.cs" 2>&1 | Out-Null
if (-not (Test-Path $hostExe) -or -not (Test-Path $childExe)) {
    Write-Fail "could not build the C# harnesses (csc at $csc)"
    exit 1
}

# The client must take the route each case asks for, and must not believe it
# is nested in a session of whoever runs this suite.
$scrub = "TMUX","TMUX_PANE","PSMUX_SESSION","PSMUX_VT_INPUT","TERM_PROGRAM","WEZTERM_PANE","TERMINAL_EMULATOR","SSH_CONNECTION","SSH_CLIENT","SSH_TTY"
# Remove-Item, not SetEnvironmentVariable($v, $null): PowerShell passes $null
# to a string parameter as "", which leaves SSH_CONNECTION defined but empty,
# and the client counts a defined SSH_CONNECTION as an ssh session and takes
# the VT input route on every case.
$saved = @{}
foreach ($v in $scrub) {
    $saved[$v] = [Environment]::GetEnvironmentVariable($v)
    Remove-Item "Env:\$v" -EA SilentlyContinue
}

$ascii = "Your paste text here, a few hundred characters long. " * 8
$uni = $ascii.Substring(0, 200) + [char]0x2014 + $ascii.Substring(200)
$asciiFile = Join-Path $root "ascii.txt"; [IO.File]::WriteAllText($asciiFile, $ascii, (New-Object Text.UTF8Encoding $false))
$uniFile = Join-Path $root "uni.txt"; [IO.File]::WriteAllText($uniFile, $uni, (New-Object Text.UTF8Encoding $false))

$OPEN = "1b 5b 32 30 30 7e"; $CLOSE = "1b 5b 32 30 31 7e"

function Invoke-Case([string]$Tag, [string]$TextFile, [string]$Text, [int]$Delay, [bool]$Vt) {
    $ns = "i766_" + $Tag + "_" + (Get-Random -Maximum 999999)
    $log = Join-Path $root "$ns.log"
    if ($Vt) { $env:PSMUX_VT_INPUT = "1" } else { Remove-Item Env:\PSMUX_VT_INPUT -EA SilentlyContinue }
    $cmd = "`"$PSMUX`" -L $ns new-session -A -s p -- `"$childExe`" `"$log`""
    $p = Start-Process -FilePath $hostExe -ArgumentList @("`"$log`"", "`"$TextFile`"", "$Delay", $cmd) -PassThru -WindowStyle Hidden
    if (-not $p.WaitForExit(40000)) { try { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } catch {} }
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    Remove-Item Env:\PSMUX_VT_INPUT -EA SilentlyContinue
    $res = [pscustomobject]@{ Ready = $false; CrAfter = $false; HeadLeak = $null; Exact = $false; Hex = "" }
    if (-not (Test-Path $log)) { return $res }
    $lines = Get-Content $log
    $res.Ready = [bool]($lines | Where-Object { $_ -match " write paste$" })
    $hex = (($lines | Where-Object { $_ -match "^\d+ stdin \d+B " } | ForEach-Object { ($_ -split " ", 4)[3] }) -join " ").Trim()
    $res.Hex = $hex
    if ($env:PSMUX_TEST_VERBOSE) { $lines | ForEach-Object { if ($_.Length -gt 110) { Write-Info ($_.Substring(0, 60) + " ... " + $_.Substring($_.Length - 40)) } else { Write-Info $_ } } }
    $o = $hex.IndexOf($OPEN); $c = $hex.IndexOf($CLOSE); $cr = $hex.LastIndexOf("0d")
    if ($o -ge 0 -and $c -gt $o) {
        $res.CrAfter = ($cr -gt $c)
        $res.HeadLeak = if ($o -gt 0) { $hex.Substring(0, $o).Trim() } else { "" }
        $innerHex = $hex.Substring($o + $OPEN.Length, $c - $o - $OPEN.Length).Trim()
        $bytes = [byte[]]@($innerHex -split " " | Where-Object { $_ } | ForEach-Object { [Convert]::ToByte($_, 16) })
        $inner = [Text.Encoding]::UTF8.GetString($bytes)
        $res.Exact = ($inner -eq $Text)
    }
    return $res
}

try {
    # ── 1. console input route ─────────────────────────────────────────────
    Write-Host "`n[1] console input route (default): CR 200 and 250 ms after the paste" -ForegroundColor Cyan
    $leaks = 0; $runs = 0
    foreach ($delay in 200, 250) {
        foreach ($kind in "ascii", "uni") {
            $file = if ($kind -eq "ascii") { $asciiFile } else { $uniFile }
            $text = if ($kind -eq "ascii") { $ascii } else { $uni }
            $after = 0; $ready = 0; $dash = 0
            for ($i = 1; $i -le $Reps; $i++) {
                $r = Invoke-Case "con$delay$kind$i" $file $text $delay $false
                if (-not $r.Ready) { continue }
                $ready++; $runs++
                if ($r.CrAfter) { $after++ }
                if ($r.HeadLeak) { $leaks++ }
                if ($kind -eq "uni" -and $r.Hex -match "e2 80 94") { $dash++ }
            }
            if ($ready -eq 0) { Write-Fail "[$delay ms $kind] the pane never came up"; continue }
            if ($after -eq $ready) { Write-Pass "[$delay ms $kind] the CR arrived after ESC[201~ in $after/$ready runs" }
            else { Write-Fail "[$delay ms $kind] the CR arrived after ESC[201~ in only $after/$ready runs (inside the paste otherwise)" }
            if ($kind -eq "uni") {
                if ($dash -eq $ready) { Write-Pass "[$delay ms uni] the em dash (an Alt code record) reached the pane in $dash/$ready runs" }
                else { Write-Fail "[$delay ms uni] the em dash reached the pane in only $dash/$ready runs" }
            }
        }
    }
    # A shifted first character is held 3 ms as the head of a paste; a host
    # slower than that can still let it out first, so this is reported, not
    # judged. Before the fix it leaked in every run.
    Write-Info "first character typed ahead of ESC[200~ in $leaks/$runs runs on this route"

    # ── 2. VT input route ─────────────────────────────────────────────────
    Write-Host "`n[2] PSMUX_VT_INPUT=1: the paste is forwarded as written" -ForegroundColor Cyan
    foreach ($delay in 50, 150) {
        foreach ($kind in "ascii", "uni") {
            $file = if ($kind -eq "ascii") { $asciiFile } else { $uniFile }
            $text = if ($kind -eq "ascii") { $ascii } else { $uni }
            $after = 0; $ready = 0; $exact = 0; $clean = 0
            for ($i = 1; $i -le $Reps; $i++) {
                $r = Invoke-Case "vt$delay$kind$i" $file $text $delay $true
                if (-not $r.Ready) { continue }
                $ready++
                if ($r.CrAfter) { $after++ }
                if ($r.Exact) { $exact++ }
                if ($r.HeadLeak -eq "") { $clean++ }
            }
            if ($ready -eq 0) { Write-Fail "[vt $delay ms $kind] the pane never came up"; continue }
            if ($after -eq $ready) { Write-Pass "[vt $delay ms $kind] the CR arrived after ESC[201~ in $after/$ready runs" }
            else { Write-Fail "[vt $delay ms $kind] the CR arrived after ESC[201~ in only $after/$ready runs" }
            if ($exact -eq $ready) { Write-Pass "[vt $delay ms $kind] the bracketed text is byte exact in $exact/$ready runs" }
            else { Write-Fail "[vt $delay ms $kind] the bracketed text is byte exact in only $exact/$ready runs" }
            if ($clean -eq $ready) { Write-Pass "[vt $delay ms $kind] nothing reached the pane ahead of ESC[200~ in $clean/$ready runs" }
            else { Write-Fail "[vt $delay ms $kind] characters reached the pane ahead of ESC[200~ in $($ready - $clean)/$ready runs" }
        }
    }
}
finally {
    foreach ($v in $scrub) {
        if ($null -ne $saved[$v]) { Set-Item "Env:\$v" $saved[$v] } else { Remove-Item "Env:\$v" -EA SilentlyContinue }
    }
    Remove-Item -Recurse -Force $root -EA SilentlyContinue
}

Write-Host ""
Write-Host "=== Results: $($script:Pass) passed, $($script:Fail) failed, $($script:Skip) skipped ===" -ForegroundColor $(if ($script:Fail -eq 0) { "Green" } else { "Red" })
exit $(if ($script:Fail -eq 0) { 0 } else { 1 })
