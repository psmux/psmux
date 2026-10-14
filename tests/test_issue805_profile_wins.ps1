# test_issue805_profile_wins.ps1
# Issue #805: psmux's pwsh init installed `function Global:claude` and the
# `env` shim AFTER dot sourcing the user's profile, so a `claude` or `env`
# the user defined in their profile was silently replaced in every pane.
# Also CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1 was forced on every pane even
# when the user had set the variable themselves.
#
# Isolation: the user's real profile is never touched. The script copies the
# pwsh install into a temp dir; pwsh resolves $PSHOME from its exe location,
# so $PROFILE.AllUsersAllHosts of the copy is a temp file this test owns.
# Everything runs under a unique -L namespace and a temp PSMUX_DATA_DIR.
#
# Usage: pwsh -NoProfile -File tests\test_issue805_profile_wins.ps1
#   PSMUX_TEST_BIN selects the psmux binary (default: psmux on PATH).

$ErrorActionPreference = 'Continue'
$script:Passed = 0
$script:Failed = 0
function Pass($m) { Write-Host "  PASS: $m" -ForegroundColor Green; $script:Passed++ }
function Fail($m) { Write-Host "  FAIL: $m" -ForegroundColor Red; $script:Failed++ }
function Info($m) { Write-Host "  INFO: $m" -ForegroundColor Cyan }

$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
Info "psmux: $PSMUX"

# Private pwsh copy (cached between runs).
$realPwsh = (Get-Command pwsh -EA Stop).Source
$realHome = & $realPwsh -NoLogo -NoProfile -Command '$PSHOME'
$pwshDir = Join-Path $env:TEMP 'psmux_test805_pwsh'
if (-not (Test-Path (Join-Path $pwshDir 'pwsh.exe'))) {
    Info "copying $realHome to $pwshDir"
    robocopy $realHome $pwshDir /E /NFL /NDL /NJH /NJS /NP /R:0 /W:0 | Out-Null
}
$PWSH = Join-Path $pwshDir 'pwsh.exe'
$profileFile = & $PWSH -NoLogo -NoProfile -Command '$PROFILE.AllUsersAllHosts'
if ($profileFile -notlike "$pwshDir*") { Write-Error "copied pwsh does not use its own PSHOME ($profileFile)"; exit 2 }

$ns = "t805_$([guid]::NewGuid().ToString('N').Substring(0,8))"
$work = Join-Path $env:TEMP $ns
New-Item -ItemType Directory -Force $work | Out-Null
$env:PSMUX_DATA_DIR = Join-Path $work 'data'
$before = (& psmux ls 2>&1 | Out-String).Trim()
# The server inherits this process's environment; start from a clean slate so
# the default case measures psmux, not whatever launched the test.
Remove-Item Env:CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS -EA 0

function Write-Profile([string]$body) {
    Set-Content -Path $profileFile -Value $body -Encoding UTF8
}

# Open a window running the private pwsh and report what the pane sees.
$script:n = 0
function Probe-Pane([string]$label) {
    $script:n++
    $out = Join-Path $work "probe$($script:n).txt"
    & $PSMUX -L $ns new-window -t s805 -n "w$($script:n)" $PWSH 2>&1 | Out-Null
    $tgt = "s805:w$($script:n)"
    $deadline = (Get-Date).AddSeconds(30)
    do {
        Start-Sleep -Milliseconds 200
        $cap = & $PSMUX -L $ns capture-pane -p -t $tgt 2>&1 | Out-String
    } while ($cap -notmatch 'PS [A-Z]:\\' -and (Get-Date) -lt $deadline)
    $cmd = "& { `$r = [ordered]@{}; foreach (`$nm in 'claude','env') { `$c = Get-Command `$nm -EA 0; `$r[`$nm+'_type'] = if (`$c) { [string]`$c.CommandType } else { 'none' }; `$r[`$nm+'_def'] = if (`$c) { [string]`$c.Definition } else { '' } }; `$r['teams'] = if (Test-Path Env:CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS) { `$env:CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS } else { '<unset>' }; `$r['envrun'] = try { (env A805=shim_ok cmd /c 'echo %A805%' 2>`$null) -join '' } catch { 'error' }; `$r | ConvertTo-Json -Compress | Set-Content -Encoding UTF8 '$out' }"
    & $PSMUX -L $ns send-keys -t $tgt -l $cmd 2>&1 | Out-Null
    & $PSMUX -L $ns send-keys -t $tgt Enter 2>&1 | Out-Null
    $deadline = (Get-Date).AddSeconds(20)
    while (-not (Test-Path $out) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 200 }
    Start-Sleep -Milliseconds 200
    if (-not (Test-Path $out)) { Fail "$label : pane never answered"; return $null }
    $j = Get-Content $out -Raw | ConvertFrom-Json
    Info "$label : claude=$($j.claude_type) env=$($j.env_type) teams=$($j.teams)"
    return $j
}

function Probe-Outside {
    $j = & $PWSH -NoLogo -Command "`$r=[ordered]@{}; foreach (`$nm in 'claude','env') { `$c = Get-Command `$nm -EA 0; `$r[`$nm+'_type'] = if (`$c) { [string]`$c.CommandType } else { 'none' }; `$r[`$nm+'_def'] = if (`$c) { [string]`$c.Definition } else { '' } }; `$r | ConvertTo-Json -Compress"
    return ($j | ConvertFrom-Json)
}

$wrapperMark = 'CommandType Application'   # body of psmux's claude wrapper (#475)

try {
    Write-Profile ''
    & $PSMUX -L $ns new-session -d -s s805 $PWSH 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    & $PSMUX -L $ns has-session -t s805 2>$null
    if ($LASTEXITCODE -ne 0) { Write-Error "could not start session"; exit 2 }

    Write-Host "`n[1] user functions claude and env in the profile"
    Write-Profile "function claude { 'USER-CLAUDE-WRAPPER' }`nfunction env { 'USER-ENV-FUNCTION' }"
    $o = Probe-Outside
    Info "outside psmux: claude def = $($o.claude_def.Trim())"
    $p = Probe-Pane 'functions'
    if ($p) {
        if ($p.claude_def -match 'USER-CLAUDE-WRAPPER') { Pass 'pane keeps the user claude function' } else { Fail "pane claude is not the user's: $($p.claude_def.Trim().Substring(0, [Math]::Min(80, $p.claude_def.Trim().Length)))" }
        if ($p.env_def -match 'USER-ENV-FUNCTION') { Pass 'pane keeps the user env function' } else { Fail "pane env is not the user's" }
        if ($p.claude_def -eq $o.claude_def) { Pass 'claude definition identical inside and outside psmux' } else { Fail 'claude definition differs inside vs outside psmux' }
    }

    Write-Host "`n[2] user aliases claude and env in the profile"
    Write-Profile "Set-Alias claude Get-Date`nSet-Alias env Get-Location"
    $p = Probe-Pane 'aliases'
    if ($p) {
        if ($p.claude_type -eq 'Alias' -and $p.claude_def -eq 'Get-Date') { Pass 'pane keeps the user claude alias' } else { Fail "claude resolves to $($p.claude_type)" }
        if ($p.env_type -eq 'Alias' -and $p.env_def -eq 'Get-Location') { Pass 'pane keeps the user env alias' } else { Fail "env resolves to $($p.env_type)" }
    }

    Write-Host "`n[3] user filters claude and env in the profile"
    Write-Profile "filter claude { 'USER-CLAUDE-FILTER' }`nfilter env { 'USER-ENV-FILTER' }"
    $p = Probe-Pane 'filters'
    if ($p) {
        if ($p.claude_def -match 'USER-CLAUDE-FILTER') { Pass 'pane keeps the user claude filter' } else { Fail "claude is not the user's filter" }
        if ($p.env_def -match 'USER-ENV-FILTER') { Pass 'pane keeps the user env filter' } else { Fail "env is not the user's filter" }
    }

    Write-Host "`n[4] no user definitions: psmux wrappers still installed (#475)"
    Write-Profile ''
    $p = Probe-Pane 'empty profile'
    if ($p) {
        if ($p.claude_type -eq 'Function' -and $p.claude_def -match $wrapperMark) { Pass 'claude wrapper installed when the user has none' } else { Fail "claude wrapper missing ($($p.claude_type))" }
        if ($p.env_type -eq 'Function' -and $p.env_def -match '_shebang') { Pass 'env shim installed when the user has none' } else { Fail "env shim missing ($($p.env_type))" }
        if ($p.envrun -eq 'shim_ok') { Pass 'env shim runs env VAR=val command' } else { Fail "env shim run gave '$($p.envrun)'" }
        if ($p.teams -eq '1') { Pass 'CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS defaults to 1' } else { Fail "agent teams default is $($p.teams)" }
    }

    Write-Host "`n[5] user set CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=0 in the global environment"
    & $PSMUX -L $ns set-environment -g CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS 0 2>&1 | Out-Null
    $p = Probe-Pane 'set-environment 0'
    if ($p) { if ($p.teams -eq '0') { Pass 'user value 0 kept' } else { Fail "user value replaced: teams=$($p.teams)" } }
    & $PSMUX -L $ns set-environment -gu CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS 2>&1 | Out-Null

    Write-Host "`n[6] claude-code-fix-tty off: no wrapper, no agent teams default"
    & $PSMUX -L $ns set -g claude-code-fix-tty off 2>&1 | Out-Null
    $p = Probe-Pane 'fix-tty off'
    if ($p) {
        if ($p.claude_type -ne 'Function') { Pass 'no claude wrapper with fix-tty off' } else { Fail 'claude wrapper present with fix-tty off' }
        if ($p.teams -eq '<unset>') { Pass 'agent teams not forced with fix-tty off' } else { Fail "agent teams forced with fix-tty off: $($p.teams)" }
    }
    & $PSMUX -L $ns set -g claude-code-fix-tty on 2>&1 | Out-Null

    Write-Host "`n[7] server launched from an environment with CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=0"
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    $env:CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS = '0'
    & $PSMUX -L $ns new-session -d -s s805 $PWSH 2>&1 | Out-Null
    Remove-Item Env:CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS -EA 0
    Start-Sleep -Milliseconds 500
    $p = Probe-Pane 'inherited 0'
    if ($p) { if ($p.teams -eq '0') { Pass 'inherited user value 0 kept' } else { Fail "inherited user value replaced: teams=$($p.teams)" } }
}
finally {
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    Remove-Item $profileFile -Force -EA 0
    $after = (& psmux ls 2>&1 | Out-String).Trim()
    if ($before -ne $after) { Write-Host "  WARNING: default namespace session list changed" -ForegroundColor Red }
    Remove-Item $work -Recurse -Force -EA 0
}

Write-Host "`nResults: $($script:Passed) passed, $($script:Failed) failed"
exit $(if ($script:Failed -eq 0) { 0 } else { 1 })
