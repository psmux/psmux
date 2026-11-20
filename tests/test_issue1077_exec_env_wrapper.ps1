# Issue #1077: Claude Code 2.1.329 teammate panes die at once.
#
# Claude Code now starts each teammate pane through /bin/sh:
#   split-window ... /bin/sh -c "cd '<dir>' && exec env CLAUDECODE=1 ... '<claude.exe>' --agent-id ..."
#   respawn-pane -k -t <pane> -- /bin/sh -c "<same line>"   (after remain-on-exit failed)
# psmux took the sh wrapper off and handed `cd ... && exec env ...` to pwsh,
# which has no `exec`, so the pane died with
#   exec: The term 'exec' is not recognized as a name of a cmdlet, ...
# Under tmux sh runs the line: cd moves, env sets the variables, exec replaces
# sh with the program. psmux now does the same parts itself.
#
# Arms (each run twice: Git usr\bin off PATH and on PATH, SHELL unset):
#   1. split-window with the issue's exact line: program runs, A=1 applied
#   2. respawn-pane -k -- /bin/sh -c (the Claude Code path): same
#   3. exec without env
#   4. spaces: cd target, env value and program path all hold spaces
#   5. the line's cd wins over split-window -c (tmux runs cd after the start dir)
#   6. no shell is left between the pane and the program (exec parity)
#   7. the old form (no wrapper, no exec) still works
#   8. #{pane_start_command} still reports the line as sent
#
# Isolation: own -L namespace and PSMUX_DATA_DIR; honours PSMUX_TEST_BIN.
$ErrorActionPreference = 'Continue'
$env:PSMUX_RUN_NOPAUSE = '1'
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:Passed = 0
$script:Failed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Passed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Failed++ }

$origPath = $env:PATH
$origData = $env:PSMUX_DATA_DIR
$origShell = $env:SHELL
[Environment]::SetEnvironmentVariable('SHELL', $null, 'Process')

$spacedRoot = Join-Path $env:TEMP 'psmux1077\Dir With Space'
New-Item -ItemType Directory -Force -Path $spacedRoot | Out-Null
$spacedExe = Join-Path $spacedRoot 'my teammate.exe'
Copy-Item -Force "$env:SystemRoot\System32\cmd.exe" $spacedExe

function Get-PaneText([string]$ns, [string]$pane, [string]$want) {
    $t = ''
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 250
        $t = (& $PSMUX -L $ns capture-pane -p -S - -t $pane 2>&1 | Out-String)
        if ($want -and $t -match $want) { return $t }
    }
    return $t
}

function Invoke-Arms([bool]$gitOnPath) {
    $label = if ($gitOnPath) { 'Git usr\bin ON PATH' } else { 'Git usr\bin OFF PATH' }
    Write-Host "`n=== Issue #1077 ($label) ===" -ForegroundColor Cyan
    if ($gitOnPath) {
        $git = 'C:\Program Files\Git\usr\bin'
        if (-not (Test-Path "$git\sh.exe")) { Write-Host "  [SKIP] Git usr\bin not installed" -ForegroundColor Yellow; return }
        $env:PATH = "$git;$origPath"
    } else {
        $env:PATH = (($origPath -split ';') | Where-Object { $_ -notmatch 'Git.(usr.)?bin' }) -join ';'
    }
    $ns = 'test1077_' + [Guid]::NewGuid().ToString('N').Substring(0, 8)
    $env:PSMUX_DATA_DIR = Join-Path $env:TEMP ('psmux1077-' + $ns)
    New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null

    & $PSMUX -L $ns new-session -d -s s1077 -x 200 -y 50 -c "$env:USERPROFILE"
    Start-Sleep -Seconds 2
    & $PSMUX -L $ns has-session -t s1077 2>$null
    if ($LASTEXITCODE -ne 0) { Write-Fail "session did not start"; return }
    & $PSMUX -L $ns set-option -g remain-on-exit on | Out-Null

    # Arm 1: the issue's split-window line.
    $line1 = "cd 'C:\Windows' && exec env A=1 cmd /k echo OK-NEW %A%"
    $p1 = (& $PSMUX -L $ns new-window -d -P -F '#{pane_id}' -c "$env:USERPROFILE" /bin/sh -c $line1 | Out-String).Trim()
    $t1 = Get-PaneText $ns $p1 'OK-NEW 1'
    if ($t1 -match "exec.*not recognized|'exec' is not") { Write-Fail "arm 1: pwsh got the exec text: $($t1.Trim())" }
    elseif ($t1 -match 'OK-NEW 1') { Write-Pass "arm 1: split-window /bin/sh -c exec env line runs the program with A=1" }
    else { Write-Fail "arm 1: unexpected pane text: [$($t1.Trim())]" }
    if ($t1 -match 'C:\\Windows>') { Write-Pass "arm 1: the program started in the cd target" }
    else { Write-Fail "arm 1: prompt not in C:\Windows: [$($t1.Trim())]" }
    $dead1 = (& $PSMUX -L $ns display -p -t $p1 '#{pane_dead}' | Out-String).Trim()
    if ($dead1 -eq '0') { Write-Pass "arm 1: pane is alive" } else { Write-Fail "arm 1: pane died (pane_dead=$dead1)" }

    # Arm 2: respawn-pane -k -- /bin/sh -c, after remain-on-exit failed.
    $p2 = (& $PSMUX -L $ns new-window -d -P -F '#{pane_id}' | Out-String).Trim()
    Start-Sleep -Seconds 1
    & $PSMUX -L $ns set-option -p -t $p2 remain-on-exit failed | Out-Null
    & $PSMUX -L $ns respawn-pane -k -t $p2 -- /bin/sh -c "cd 'C:\Windows' && exec env A=1 B=two cmd /k echo OK-RESP %A% %B%"
    $t2 = Get-PaneText $ns $p2 'OK-RESP 1 two'
    if ($t2 -match 'OK-RESP 1 two') { Write-Pass "arm 2: respawn-pane -- /bin/sh -c exec env line runs with both variables" }
    else { Write-Fail "arm 2: unexpected pane text: [$($t2.Trim())]" }

    # Arm 3: exec without env.
    $p3 = (& $PSMUX -L $ns new-window -d -P -F '#{pane_id}' /bin/sh -c "cd 'C:\Windows' && exec cmd /k echo OK-EXEC-ONLY" | Out-String).Trim()
    $t3 = Get-PaneText $ns $p3 'OK-EXEC-ONLY'
    if ($t3 -match 'OK-EXEC-ONLY') { Write-Pass "arm 3: exec without env runs the program" }
    else { Write-Fail "arm 3: unexpected pane text: [$($t3.Trim())]" }

    # Arm 4: spaces everywhere.
    $line4 = "cd '$spacedRoot' && exec env XV='two words' '$spacedExe' /k echo OK-SP [%XV%]"
    $p4 = (& $PSMUX -L $ns new-window -d -P -F '#{pane_id}' /bin/sh -c $line4 | Out-String).Trim()
    $t4 = Get-PaneText $ns $p4 'OK-SP \[two words\]'
    if ($t4 -match 'OK-SP \[two words\]') { Write-Pass "arm 4: spaced program path and spaced env value survive" }
    else { Write-Fail "arm 4: unexpected pane text: [$($t4.Trim())]" }
    if ($t4 -match [regex]::Escape("$spacedRoot>")) { Write-Pass "arm 4: spaced cd target honoured" }
    else { Write-Fail "arm 4: prompt not in the spaced dir: [$($t4.Trim())]" }

    # Arm 5: cd wins over split-window -c.
    $p5 = (& $PSMUX -L $ns split-window -d -t s1077:0 -P -F '#{pane_id}' -c "$env:TEMP" /bin/sh -c "cd 'C:\Windows\System32' && exec env A=1 cmd /k echo OK-CD" | Out-String).Trim()
    $t5 = Get-PaneText $ns $p5 'System32>'
    if ($t5 -match 'C:\\Windows\\System32>') { Write-Pass "arm 5: the line's cd wins over -c, as under tmux" }
    else { Write-Fail "arm 5: pane not in the cd target: [$($t5.Trim())]" }

    # Arm 6: the pane process is the program itself (exec replaced the shell).
    $pid1 = (& $PSMUX -L $ns display -p -t $p1 '#{pane_pid}' | Out-String).Trim()
    $proc = Get-CimInstance Win32_Process -Filter "ProcessId=$pid1" -EA SilentlyContinue
    if ($proc -and $proc.Name -ieq 'cmd.exe') { Write-Pass "arm 6: pane process is cmd.exe itself (no shell left in between)" }
    else { Write-Fail "arm 6: pane process is [$($proc.Name)] pid $pid1" }

    # Arm 7: the old form keeps working.
    $p7 = (& $PSMUX -L $ns new-window -d -P -F '#{pane_id}' "cd 'C:\Windows' && env A=1 cmd /k echo OK-OLD %A%" | Out-String).Trim()
    $t7 = Get-PaneText $ns $p7 'OK-OLD 1'
    if ($t7 -match 'OK-OLD 1') { Write-Pass "arm 7: old form (no wrapper, no exec) still runs with A=1" }
    else { Write-Fail "arm 7: unexpected pane text: [$($t7.Trim())]" }

    # Arm 8: pane_start_command still reports what was sent.
    $sc = (& $PSMUX -L $ns display -p -t $p1 '#{pane_start_command}' | Out-String).Trim()
    if ($sc -match 'exec env A=1' -and $sc -match 'bin/sh') { Write-Pass "arm 8: pane_start_command keeps the line: $sc" }
    else { Write-Fail "arm 8: pane_start_command is [$sc]" }

    & $PSMUX -L $ns kill-server 2>$null | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $env:PSMUX_DATA_DIR -EA SilentlyContinue
}

try {
    Invoke-Arms $false
    Invoke-Arms $true
} finally {
    $env:PATH = $origPath
    $env:PSMUX_DATA_DIR = $origData
    if ($origShell) { $env:SHELL = $origShell }
    Remove-Item -Recurse -Force (Join-Path $env:TEMP 'psmux1077') -EA SilentlyContinue
}

Write-Host "`nResults: $script:Passed passed, $script:Failed failed"
if ($script:Failed -gt 0) { exit 1 } else { exit 0 }
