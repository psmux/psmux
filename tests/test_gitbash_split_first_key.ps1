# Git bash: the first key typed after a split (or any pane resize) must reach
# the shell.
#
# The Cygwin console runtime behind Git bash and MSYS2 loses the first input
# record that wakes its read after its console was resized. A split resizes
# the claimed warm spare (spawned at the full window size) and the pane it
# splits, so the first key typed there went missing: "echo X" ran as "cho X"
# in 5 of 10 runs on 9a2e9352 (bash: cho: command not found). psmux now
# writes a Shift press to a Cygwin runtime pane right after resizing it, and
# that press is the record that gets dropped.
#
# Each round starts a visible attached session with Git bash as default-shell
# in its own -L namespace and PSMUX_DATA_DIR, splits, types at once, and
# checks the command's OUTPUT line. It then resizes the first (settled) pane
# and types there at once.
#
# Usage: pwsh tests\test_gitbash_split_first_key.ps1 [-Rounds 10]
#        PSMUX_TEST_BIN picks the binary.

#        -CheckPrompt also checks the prompt line is clean after each resize
#        (it adds 300 ms per check, which hides part of the race it measures).
param([int]$Rounds = 10, [switch]$CheckPrompt)

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$gitBash = "C:\Program Files\Git\bin\bash.exe"
$script:Passed = 0
$script:Failed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Passed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Failed++ }

if (-not (Test-Path $gitBash)) {
    Write-Host "  [SKIP] Git for Windows not installed" -ForegroundColor DarkYellow
    exit 0
}

# Before Windows 11 the console host gives no record psmux can send that
# absorbs the loss, so psmux sends nothing there and the loss remains.
if ([Environment]::OSVersion.Version.Build -lt 22000) {
    Write-Host "  [SKIP] Windows build $([Environment]::OSVersion.Version.Build): the first key after a resize is still lost by the Cygwin runtime here (known; no record psmux can send prevents it on this build)" -ForegroundColor DarkYellow
    exit 0
}

function Wait-Prompt($ns, $target) {
    for ($i = 0; $i -lt 150; $i++) {
        if ((& $PSMUX -L $ns capture-pane -t $target -p 2>&1 | Out-String) -match '\$\s*$') { return $true }
        Start-Sleep -Milliseconds 100
    }
    return $false
}

# The prompt line must hold nothing but the prompt: the Shift press psmux
# writes after a resize must never show up as text (a console host that does
# not parse win32-input-mode would otherwise echo it).
function Test-CleanPrompt($ns, $target, $label) {
    Start-Sleep -Milliseconds 300
    $lines = @((& $PSMUX -L $ns capture-pane -t $target -p 2>&1 | Out-String) -split "`r?`n" | Where-Object { $_.Trim() })
    $last = if ($lines.Count) { $lines[-1] } else { "" }
    if ($last.Trim() -eq '$') { Write-Pass "$label : prompt line clean" }
    else { Write-Fail "$label : stray text on the prompt line: '$last'" }
}

# Types "echo <marker>" into $target at once and waits for the output line.
function Test-FirstKey($ns, $target) {
    $m = "GBK" + (Get-Random -Minimum 10000 -Maximum 99999)
    & $PSMUX -L $ns send-keys -t $target "echo $m" Enter
    $cap = ""
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 200
        $cap = (& $PSMUX -L $ns capture-pane -t $target -p 2>&1 | Out-String)
        if ($cap -match "(?m)^$m") { return @{ ok = $true; cap = $cap } }
    }
    return @{ ok = $false; cap = $cap }
}

Write-Host "`n=== Git bash first key after split / resize ===" -ForegroundColor Cyan
Write-Host "  binary: $PSMUX" -ForegroundColor DarkGray

# Run from inside a psmux pane, the session this suite starts must not look
# nested to psmux.
foreach ($v in 'TMUX', 'TMUX_PANE', 'PSMUX_SESSION') { Remove-Item "env:$v" -EA SilentlyContinue }

$splitLost = 0; $resizeLost = 0
for ($r = 0; $r -lt $Rounds; $r++) {
    $ns = "gbkey_${PID}_$r"
    $dataDir = Join-Path $env:TEMP "gbkey_${PID}_$r"
    New-Item -ItemType Directory -Force $dataDir | Out-Null
    $conf = Join-Path $dataDir "psmux.conf"
    "set -g default-shell `"$gitBash`"" | Set-Content -Path $conf -Encoding UTF8
    $env:PSMUX_DATA_DIR = $dataDir
    $env:PSMUX_CONFIG_FILE = $conf
    $proc = Start-Process -FilePath $PSMUX -ArgumentList "-L",$ns,"new-session","-s","t" -PassThru
    $env:PSMUX_CONFIG_FILE = $null
    try {
        # Let the first pane and the warm spare reach their prompts.
        Start-Sleep -Seconds 6
        $dir = if ($r % 2 -eq 0) { "-h" } else { "-v" }
        & $PSMUX -L $ns split-window $dir -t t 2>&1 | Out-Null
        if (-not (Wait-Prompt $ns "t")) { Write-Fail "round $r ($dir): split pane never showed a prompt"; continue }
        if ($CheckPrompt) { Test-CleanPrompt $ns "t" "round $r split $dir" }
        $res = Test-FirstKey $ns "t"
        if ($res.ok) { Write-Pass "round $r split $dir : first key arrived" }
        else {
            $splitLost++
            Write-Fail "round $r split $dir : first key lost. capture:`n$(($res.cap -split "`n" | Where-Object { $_.Trim() } | Select-Object -Last 5) -join "`n")"
        }
        # The pane that was split is now settled; resize it and type at once.
        & $PSMUX -L $ns resize-pane -t t:0.0 -x 30 2>&1 | Out-Null
        & $PSMUX -L $ns resize-pane -t t:0.0 -y 8 2>&1 | Out-Null
        if ($CheckPrompt) { Test-CleanPrompt $ns "t:0.0" "round $r resize-pane" }
        $res = Test-FirstKey $ns "t:0.0"
        if ($res.ok) { Write-Pass "round $r resize-pane: first key arrived" }
        else {
            $resizeLost++
            Write-Fail "round $r resize-pane: first key lost. capture:`n$(($res.cap -split "`n" | Where-Object { $_.Trim() } | Select-Object -Last 5) -join "`n")"
        }
    } finally {
        & $PSMUX -L $ns kill-server 2>&1 | Out-Null
        Start-Sleep -Milliseconds 300
        if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue }
        $env:PSMUX_DATA_DIR = $null
        Remove-Item $dataDir -Recurse -Force -EA SilentlyContinue
    }
}

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  split first key lost:  $splitLost / $Rounds"
Write-Host "  resize first key lost: $resizeLost / $Rounds"
Write-Host "  Passed: $($script:Passed)  Failed: $($script:Failed)"
exit $script:Failed
