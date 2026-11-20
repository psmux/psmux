# PR #1060: Shift+Enter reaches a console key record reader (PSReadLine) as
# Shift+Enter, through send-keys and through a real attached client.
#
# Before: every Shift+Enter left psmux as the legacy bytes ESC CR. ConPTY
# turns ESC CR into ONE Alt+Enter record, so PSReadLine (which reads
# INPUT_RECORDs) never saw Shift and its Shift+Enter AddLine binding did not
# fire: `echo one` S-Enter `echo two` came out as the single line
# "echo oneecho two". A VT byte reader (node, Claude Code) must still get the
# ESC CR it maps to meta+return, so that is pinned here too.
#
# Isolation: own -L namespace per server, own PSMUX_DATA_DIR and config, the
# client is started by this script in its own console and stopped by its PID,
# cleanup is `-L <ns> kill-server`. PSMUX_TEST_BIN (or PSMUX_EXE) picks the
# binary.

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$PSMUX = $env:PSMUX_TEST_BIN
if (-not $PSMUX) { $PSMUX = $env:PSMUX_EXE }
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }
$env:PSMUX_RUN_NOPAUSE = "1"

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow }

$Work = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_1060_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $Work | Out-Null
$env:PSMUX_DATA_DIR = $Work
$env:PSMUX_NO_WARM = "1"
$cfg = Join-Path $Work "psmux.conf"
Set-Content -Path $cfg -Value "set -g default-command 'pwsh -NoProfile -NoLogo'"
$env:PSMUX_CONFIG_FILE = $cfg
$script:Namespaces = @()
$script:Started = @()

function New-Ns {
    $ns = "r16pr1060_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    $script:Namespaces += $ns
    return $ns
}
function Capture($ns) { (& $PSMUX -L $ns capture-pane -p -t s 2>&1) -join "`n" }
function Wait-Prompt($ns) {
    for ($i = 0; $i -lt 40; $i++) {
        if ((Capture $ns) -match 'PS [A-Z]:\\') { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$INJ = Join-Path $Work "injector.exe"
& $csc /nologo /optimize "/out:$INJ" (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null

Write-Host "`n=== PR #1060: Shift+Enter reaches PSReadLine as Shift+Enter ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((& $PSMUX -V) -join ' ')"

try {
    Write-Host "`n[1] send-keys 'echo one' S-Enter 'echo two' Enter in a PSReadLine pane" -ForegroundColor Yellow
    $ns = New-Ns
    & $PSMUX -L $ns new-session -d -s s -x 100 -y 20 2>&1 | Out-Null
    if (-not (Wait-Prompt $ns)) { Write-Fail "pwsh prompt never appeared" }
    else {
        Start-Sleep -Milliseconds 800
        & $PSMUX -L $ns send-keys -t s 'echo one' S-Enter 'echo two' 2>&1 | Out-Null
        Start-Sleep -Milliseconds 800
        $before = Capture $ns
        & $PSMUX -L $ns send-keys -t s Enter 2>&1 | Out-Null
        Start-Sleep -Milliseconds 1500
        $after = Capture $ns
        if ($before -match 'oneecho two') { Write-Fail "Shift+Enter was lost, the edit line is one line: 'echo oneecho two'" }
        elseif ($before -match '(?m)^>> echo two') { Write-Pass "Shift+Enter added a continuation line ('>> echo two')" }
        else { Write-Fail "no continuation line before Enter:`n$before" }
        $lines = @($after -split "`n" | ForEach-Object { $_.TrimEnd() })
        if (($lines -contains 'one') -and ($lines -contains 'two')) { Write-Pass "both commands ran, printing 'one' and 'two' on their own lines" }
        else { Write-Fail "output after Enter:`n$after" }
    }
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null

    Write-Host "`n[2] send-keys -l 'S-Enter' stays text" -ForegroundColor Yellow
    $ns = New-Ns
    & $PSMUX -L $ns new-session -d -s s -x 100 -y 20 2>&1 | Out-Null
    if (-not (Wait-Prompt $ns)) { Write-Fail "pwsh prompt never appeared" }
    else {
        Start-Sleep -Milliseconds 800
        & $PSMUX -L $ns send-keys -t s -l 'echo S-Enter' 2>&1 | Out-Null
        Start-Sleep -Milliseconds 800
        if ((Capture $ns) -match 'echo S-Enter') { Write-Pass "literal S-Enter typed as text" } else { Write-Fail "literal S-Enter not typed: $(Capture $ns)" }
    }
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null

    Write-Host "`n[3] a real Shift+Enter key record typed into an attached client" -ForegroundColor Yellow
    if (-not (Test-Path $INJ)) { Write-Skip "injector did not compile" }
    else {
        $ns = New-Ns
        & $PSMUX -L $ns new-session -d -s s -x 100 -y 20 2>&1 | Out-Null
        if (-not (Wait-Prompt $ns)) { Write-Fail "pwsh prompt never appeared" }
        else {
            $client = Start-Process -FilePath $PSMUX -ArgumentList "-L", $ns, "attach", "-t", "s" -PassThru
            $script:Started += $client.Id
            Start-Sleep -Seconds 3
            & $INJ $client.Id "echo three{MOD:0D:000D:0010}echo four" | Out-Null
            Start-Sleep -Milliseconds 1200
            $before = Capture $ns
            & $INJ $client.Id "{ENTER}" | Out-Null
            Start-Sleep -Milliseconds 1500
            $after = Capture $ns
            if ($before -match 'threeecho four') { Write-Fail "typed Shift+Enter was lost: 'echo threeecho four'" }
            elseif ($before -match '(?m)^>> echo four') { Write-Pass "typed Shift+Enter added a continuation line ('>> echo four')" }
            else { Write-Fail "no continuation line before Enter:`n$before" }
            $lines = @($after -split "`n" | ForEach-Object { $_.TrimEnd() })
            if (($lines -contains 'three') -and ($lines -contains 'four')) { Write-Pass "both typed commands ran" }
            else { Write-Fail "output after Enter:`n$after" }
            Stop-Process -Id $client.Id -Force -EA SilentlyContinue
        }
        & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    }

    Write-Host "`n[4] a VT byte reader (node raw mode, as Claude Code) keeps its bytes" -ForegroundColor Yellow
    $node = Get-Command node -EA SilentlyContinue
    if (-not $node) { Write-Skip "node is not on PATH" }
    else {
        $ns = New-Ns
        $log = Join-Path $Work 'recv.log'
        $js = Join-Path $PSScriptRoot 'ctrl_enter_recv.js'
        & $PSMUX -L $ns new-session -d -s s -x 100 -y 20 "node `"$js`" `"$log`"" 2>&1 | Out-Null
        for ($i = 0; $i -lt 40 -and -not (Test-Path $log); $i++) { Start-Sleep -Milliseconds 250 }
        Start-Sleep -Milliseconds 1000
        $want = [ordered]@{ 'S-Enter' = '1b 0d'; 'M-Enter' = '1b 0d'; 'C-Enter' = '0a'; 'Enter' = '0d' }
        foreach ($k in $want.Keys) {
            $mark = (Get-Content $log -EA SilentlyContinue).Count
            & $PSMUX -L $ns send-keys -t s $k 2>&1 | Out-Null
            Start-Sleep -Milliseconds 800
            $got = (@(Get-Content $log -EA SilentlyContinue | Select-Object -Skip $mark | Where-Object { $_ -like 'BYTES:*' }) -replace '^BYTES: ', '') -join ' '
            if ($got -eq $want[$k]) { Write-Pass "$k -> $got" } else { Write-Fail "$k expected '$($want[$k])', got '$got'" }
        }
        & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    }
}
catch { Write-Fail "exception: $_" }
finally {
    foreach ($p in $script:Started) { Stop-Process -Id $p -Force -EA SilentlyContinue }
    foreach ($ns in $script:Namespaces) { & $PSMUX -L $ns kill-server 2>&1 | Out-Null }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $Work -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor $(if ($script:TestsFailed) { 'Red' } else { 'Green' })
exit $script:TestsFailed
