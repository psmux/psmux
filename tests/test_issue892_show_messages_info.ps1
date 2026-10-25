# show-messages prints what info prints, on every route, control mode too.
#
# tmux 3.7c's default command aliases make info and server-info
# `show-messages -JT` (options-table.c). psmux keeps no message log and has no
# terminfo or job table, so show-messages prints the server-info summary
# whatever its -J, -T and -t say. Before the fix it printed nothing on the CLI,
# an empty popup at the prompt, dropped its output from a hook, and control
# mode answered `unknown command: show-messages`.
#
# Live: a real attached client, keys through WriteConsoleInput
# (tests/injector.cs) and the client's console read back (tests/conread.cs).
# Isolated: -L, and USERPROFILE, PSMUX_DATA_DIR and PSMUX_CONFIG_FILE point
# into a temp dir. PSMUX_EXE picks the binary (defaults to target\release).

$ErrorActionPreference = "Continue"
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -and $_.Name -ne 'PSMUX_EXE' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
$PSMUX = $env:PSMUX_EXE
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Check($name, $ok, $detail) { if ($ok) { Write-Pass $name } else { Write-Fail "$name $detail" } }

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_smi_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
. "$PSScriptRoot\isolated_config.ps1" -Lines @('bind M show-messages -JT')
$NS = "smi_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$S = "smi"
$S2 = "smi2"
function P { & $PSMUX -L $NS @args 2>&1 }

Write-Host "`n=== show-messages prints what info prints ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((P -V) -join ' ')"

$toolDir = Join-Path $TempHome "tools"
New-Item -ItemType Directory -Force $toolDir | Out-Null
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$INJ = Join-Path $toolDir "injector.exe"
$READ = Join-Path $toolDir "conread.exe"
& $csc /nologo /optimize /out:$INJ  (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
& $csc /nologo /optimize /out:$READ (Join-Path $PSScriptRoot "conread.cs") 2>&1 | Out-Null
if (-not ((Test-Path $INJ) -and (Test-Path $READ))) { Write-Fail "harness tools did not compile"; exit 1 }

# The info text with its uptime line taken out, which changes by the second.
function Stable($lines) { return (@($lines) | Where-Object { $_ -notmatch '^uptime: ' }) -join "`n" }

function Start-Client($sess) {
    $p = Start-Process -FilePath $PSMUX -ArgumentList "-L", $NS, "attach", "-t", $sess -PassThru
    Start-Sleep -Seconds 4
    return $p
}
function Stop-Client($p) {
    try { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } catch {}
    Start-Sleep -Milliseconds 700
}
function Send-Keys($p, $keys) {
    & $INJ $p.Id $keys | Out-Null
    Start-Sleep -Milliseconds 900
}
function Read-Screen($p) { return ([string[]](& $READ $p.Id)) -join "`n" }
# Open a client, run $open (keys, or a script block run from outside), and
# check that a popup titled $title shows this session's info text.
function Check-Popup($name, $open, $title) {
    $c = Start-Client $S
    if ($open -is [scriptblock]) { & $open $c } else { Send-Keys $c $open }
    Start-Sleep -Milliseconds 1200
    $scr = Read-Screen $c
    $ok = $scr.Contains($title) -and $scr.Contains("pid: ") -and $scr.Contains("session: $S")
    Check "$name shows the info text in a popup titled $title" $ok ""
    # The popup is the server's: close it, or the next client opens on it and
    # its prefix key goes to the popup instead of opening the prompt.
    Send-Keys $c "{ESC}{SLEEP:600}"
    Stop-Client $c
}

try {
    P new-session -d -s $S -x 120 -y 30 | Out-Null
    P new-session -d -s $S2 -x 120 -y 30 | Out-Null
    Start-Sleep -Seconds 2
    P has-session -t $S | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Fail "session did not start"; exit 1 }

    Write-Host "`n--- the CLI ---" -ForegroundColor Cyan
    $info = @(P info -t $S)
    Check "info prints six lines" ($info.Count -eq 6 -and $info[0] -match '^psmux .* \(Windows\)$' -and $info[2] -eq "session: $S") "got: $($info -join ' | ')"
    foreach ($form in @(@('show-messages'), @('showmsgs'), @('show-messages', '-J'), @('show-messages', '-T'), @('show-messages', '-JT'))) {
        $out = @(P @form -t $S)
        $rc = $LASTEXITCODE
        Check "$($form -join ' ') prints what info prints, exit 0" ($rc -eq 0 -and (Stable $out) -eq (Stable $info)) "rc=$rc got: $($out -join ' | ')"
    }

    Write-Host "`n--- the CLI, -t naming a client ---" -ForegroundColor Cyan
    $c = Start-Client $S2
    $tty = (P list-clients -t $S2 -F '#{client_name}' | Select-Object -First 1)
    $out = @(P show-messages -t $tty)
    Check "show-messages -t $tty reports the session that client is on" ((@($out) -contains "session: $S2")) "got: $($out -join ' | ')"
    Stop-Client $c
    $out = @(P show-messages -t /dev/pts/999999)
    $rc = $LASTEXITCODE
    Check "show-messages -t naming no client is not an error" ($rc -eq 0 -and (@($out) -match '^pid: ').Count -eq 1) "rc=$rc got: $($out -join ' | ')"

    Write-Host "`n--- control mode ---" -ForegroundColor Cyan
    foreach ($line in @('show-messages', 'show-messages -JT', 'showmsgs', 'info')) {
        $out = @("$line" | & $PSMUX -L $NS -C attach -t $S 2>&1)
        $text = $out -join "`n"
        $ok = ($text -notmatch '%error') -and ($text -match "(?m)^psmux .* \(Windows\)$") -and ($text -match "(?m)^session: $S$")
        Check "control mode '$line' answers with the info text" $ok "got: $(($out | Select-Object -First 6) -join ' | ')"
    }

    Write-Host "`n--- an attached client ---" -ForegroundColor Cyan
    Check-Popup ":show-messages" "^b{SLEEP:500}:{SLEEP:600}show-messages{ENTER}{SLEEP:1500}" "show-messages"
    # info is a default command-alias, so the command that runs is show-messages.
    Check-Popup ":info" "^b{SLEEP:500}:{SLEEP:600}info{ENTER}{SLEEP:1500}" "show-messages"
    Check-Popup "a prefix binding to 'show-messages -JT'" "^b{SLEEP:500}M{SLEEP:1500}" "show-messages"
    Check-Popup "an after-new-window hook running show-messages" { param($c) P set-hook -t $S after-new-window "show-messages" | Out-Null; P new-window -d -t $S | Out-Null } "show-messages"
    P set-hook -u -t $S after-new-window | Out-Null
}
finally {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-PsmuxIsolatedConfig
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $script:TestsPassed passed, $script:TestsFailed failed ===" -ForegroundColor $(if ($script:TestsFailed -eq 0) { 'Green' } else { 'Yellow' })
exit $script:TestsFailed
