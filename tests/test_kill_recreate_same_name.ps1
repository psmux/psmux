# A session name is free again the moment its session is gone.
#
# A hook on A runs `kill-session -t D`; as soon as `has-session -t D` says D is
# gone, `new-session -d -s D` must succeed, as it always does in tmux. psmux
# used to fail it with "failed to create session 'D'": the dying D server
# deleted its .port at once (so has-session said gone) but kept its session
# name mutex until it had killed every pane shell and pooled spare and exited,
# and a server cold started for the new D in that window took itself for a
# duplicate and quit. (A claimed warm standby only logged "running unguarded"
# and carried on, so the default path hit it about once in a hundred tries,
# whenever the standby had just been used up.)
#
# Each round makes the window wide and the new D a cold start on purpose: D is
# a pooled server with several windows, so its teardown takes a while, and the
# measured new-session runs with PSMUX_NO_WARM=1, which skips the standby claim.
#
# Run: pwsh -NoProfile -File tests\test_kill_recreate_same_name.ps1
# PSMUX_TEST_BIN overrides the binary under test.

$ErrorActionPreference = 'Continue'
$env:SHELL = $null
Remove-Item Env:PSMUX_NO_WARM -ErrorAction SilentlyContinue
$ExePath = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -ErrorAction Stop).Source }
$Namespace = "killrecreate$PID"
$DataDir = Join-Path ([IO.Path]::GetTempPath()) "psmux-killrecreate-$PID"
New-Item -ItemType Directory -Force $DataDir | Out-Null
$env:PSMUX_DATA_DIR = $DataDir
$Rounds = 12
$Windows = 6
$script:Passed = 0
$script:Failed = 0

function Check {
    param([bool]$Condition, [string]$Name, [string]$Detail = '')
    if ($Condition) { $script:Passed++; Write-Output "PASS: $Name" } else { $script:Failed++; Write-Output "FAIL: $Name $Detail" }
}

function P {
    param([string[]]$Arguments, [switch]$Cold)
    if ($Cold) { $env:PSMUX_NO_WARM = '1' }
    try { $output = & $ExePath -L $Namespace @Arguments 2>&1 | ForEach-Object { "$_" } }
    finally { if ($Cold) { Remove-Item Env:PSMUX_NO_WARM -ErrorAction SilentlyContinue } }
    [pscustomobject]@{ Code = $LASTEXITCODE; Text = ($output -join ' | ') }
}

function Wait-Until {
    param([scriptblock]$Condition, [int]$TimeoutMs = 10000)
    $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMs)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (& $Condition) { return $true }
        Start-Sleep -Milliseconds 25
    }
    [bool](& $Condition)
}

function Alive { param([string]$Session) (P @('has-session', '-t', $Session)).Code -eq 0 }

function Gone-Fully {
    # has-session false AND no server process left for D in this namespace.
    param([string]$Session)
    if (Alive $Session) { return $false }
    $mine = Get-CimInstance Win32_Process -Filter "ProcessId=$script:OldPid" -ErrorAction SilentlyContinue
    $null -eq $mine
}

try {
    $made = P @('new-session', '-d', '-s', 'A')
    Check ($made.Code -eq 0 -and (Wait-Until { Alive 'A' })) 'session A is up' "($($made.Text))"
    Check ((P @('set-hook', '-t', 'A', 'after-split-window', 'kill-session -t D')).Code -eq 0) "A's hook is set to kill-session -t D"

    $bad = @()
    $notEnded = 0
    $overlap = 0
    for ($i = 1; $i -le $Rounds; $i++) {
        # A pooled D (the default path) with several windows.
        $made = P @('new-session', '-d', '-s', 'D')
        if (-not ($made.Code -eq 0 -and (Wait-Until { Alive 'D' }))) { $bad += "round ${i}: setup D: $($made.Text)"; continue }
        for ($w = 1; $w -lt $Windows; $w++) { P @('new-window', '-d', '-t', 'D') | Out-Null }
        $script:OldPid = [int]((P @('display-message', '-p', '-t', 'D', '#{pid}')).Text.Trim())

        P @('split-window', '-d', '-t', 'A') | Out-Null
        if (-not (Wait-Until { -not (Alive 'D') })) { $notEnded++; continue }
        if (Get-Process -Id $script:OldPid -ErrorAction SilentlyContinue) { $overlap++ }

        $again = P @('new-session', '-d', '-s', 'D') -Cold
        $up = $again.Code -eq 0 -and (Wait-Until { Alive 'D' })
        if (-not $up) { $bad += "round ${i}: exit $($again.Code) '$($again.Text)'" }

        # Clean slate for the next round: the CLI kill waits for the server.
        P @('kill-session', '-t', 'D') | Out-Null
        Wait-Until { Gone-Fully 'D' } 15000 | Out-Null
        P @('kill-pane', '-a', '-t', 'A') | Out-Null
    }
    Write-Output "INFO: the old D server was still running when has-session said gone in $overlap of $Rounds rounds"
    Check ($notEnded -eq 0) "A's hook ended D in every one of $Rounds rounds" "($notEnded rounds D did not end)"
    Check ($bad.Count -eq 0) "new-session -s D (cold start) succeeded straight after D ended, $Rounds of $Rounds rounds" "($($bad.Count) failed: $($bad -join '; '))"
    Check (Alive 'A') 'A is still alive'

    # The duplicate guard still holds: a second D while D lives is refused, on
    # the cold path and on the default path, and the live D keeps running alone.
    $made = P @('new-session', '-d', '-s', 'D')
    Check ($made.Code -eq 0 -and (Wait-Until { Alive 'D' })) 'D is up for the duplicate checks' "($($made.Text))"
    $dup = P @('new-session', '-d', '-s', 'D') -Cold
    Check ($dup.Code -ne 0 -and $dup.Text -match 'duplicate session: D') 'a cold new-session -s D while D lives says duplicate session' "(exit $($dup.Code) '$($dup.Text)')"
    $dup = P @('new-session', '-d', '-s', 'D')
    Check ($dup.Code -ne 0 -and $dup.Text -match 'duplicate session: D') 'a default new-session -s D while D lives says duplicate session' "(exit $($dup.Code) '$($dup.Text)')"
    Check (Alive 'D') 'the live D survives both duplicate attempts'
    $names = @((P @('ls', '-F', '#{session_name}')).Text -split ' \| ')
    Check (($names | Where-Object { $_ -ceq 'D' }).Count -eq 1) 'exactly one D is listed' "(ls: $($names -join ','))"
}
finally {
    P @('kill-server') | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $DataDir -ErrorAction SilentlyContinue
}

Write-Output "Passed: $script:Passed  Failed: $script:Failed"
exit $script:Failed
