param(
    [string]$Exe = $(if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }),
    [string]$Result = $(Join-Path $env:TEMP "psmux_shared_hook_new_session_rows.txt")
)

$ErrorActionPreference = "Continue"
$script:Rows = [System.Collections.Generic.List[string]]::new()
$script:PingFails = [System.Collections.Generic.List[string]]::new()
$script:FireCount = 0
$refused = 'new-session from a hook or the config is not supported yet'
$suiteNames = 'FROMHOOK', 'TWICE', 'RP', 'NOD', 'KEEP', 'FROMCOPY', 'RELC', 'S2', 'AFTER', 'A2', 'NOCWD', 'RA', 'RB', 'RX', 'RENEW', 'QA', 'QB', 'QX', 'QNEW'

function Row($id, $state, $reason) {
    $line = "ROW $id $state $reason"
    $script:Rows.Add($line)
    $color = switch ($state) { 'PASS' { 'Green' } 'FAIL' { 'Red' } default { 'Yellow' } }
    Write-Host "  $line" -ForegroundColor $color
}

$injector = Join-Path $env:TEMP "psmux_shared_routing_injector.exe"
if (-not (Test-Path $injector)) {
    $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    & $csc /nologo /optimize "/out:$injector" (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
}

$envNames = 'PSMUX_DATA_DIR', 'PSMUX_SERVER_MODE', 'PSMUX_CONFIG_FILE', 'PSMUX_SERVER_DEBUG', 'PSMUX_PSREADLINE_HISTORY',
    'USERPROFILE', 'HOME', 'TMUX', 'TMUX_PANE', 'PSMUX_SESSION', 'PSMUX_TARGET_SESSION', 'PSMUX_SESSION_NAME'
$saved = @{}
foreach ($v in $envNames) { $saved[$v] = [Environment]::GetEnvironmentVariable($v, 'Process') }
function RestoreEnv {
    foreach ($v in $envNames) {
        if ($null -eq $saved[$v]) { Remove-Item "Env:\$v" -EA SilentlyContinue } else { Set-Item "Env:\$v" $saved[$v] }
    }
}
function ClearSwitchEnv {
    foreach ($v in 'TMUX', 'TMUX_PANE', 'PSMUX_SESSION', 'PSMUX_TARGET_SESSION', 'PSMUX_SESSION_NAME', 'PSMUX_DATA_DIR', 'PSMUX_SERVER_MODE', 'PSMUX_SERVER_DEBUG') {
        Remove-Item "Env:\$v" -EA SilentlyContinue
    }
}
ClearSwitchEnv
function DefaultLs { @(& $Exe ls -F '#{session_name} #{session_created}' 2>&1 | ForEach-Object { "$_".Trim() } | Where-Object { $_ }) }
$defaultBefore = DefaultLs

$root = Join-Path $env:TEMP "psmux_hookns_$(Get-Random)"
$profileDir = Join-Path $root 'home'
New-Item -ItemType Directory -Force $profileDir | Out-Null
$empty = Join-Path $root 'empty.conf'
'' | Set-Content $empty
$env:USERPROFILE = $profileDir
$env:HOME = $profileDir
$env:PSMUX_PSREADLINE_HISTORY = Join-Path $root 'history.txt'
$env:PSMUX_CONFIG_FILE = $empty
$env:PSMUX_SERVER_MODE = 'shared'

function NewSpace($tag) {
    $space = @{ Ns = "$tag$(Get-Random)"; Data = (Join-Path $root $tag) }
    New-Item -ItemType Directory -Force $space.Data | Out-Null
    $space
}
function Use($space) { $script:Ns = $space.Ns; $env:PSMUX_DATA_DIR = $space.Data }
function P { & $Exe -L $script:Ns @args 2>&1 }
function Has($s) { P has-session -t "=$s" | Out-Null; $LASTEXITCODE -eq 0 }
function Names { @(P ls -F '#{session_name}' | ForEach-Object { "$_".Trim() } | Where-Object { $_ }) }
function Status($s) { $d = (P dump-state -t $s) -join ''; if ($d -match '"status_message":"([^"]*)"') { $Matches[1] } else { '' } }
function ServerPid($s) { ((P display -p -t $s '#{pid}') -join '').Trim() }
function WaitFor([scriptblock]$cond, [int]$ms = 3000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 100 }
    return [bool](& $cond)
}
function Fire($s, $cmd) {
    P set-hook -t $s after-rename-window $cmd | Out-Null
    $script:FireCount++
    P rename-window -t "${s}:0" "fire$($script:FireCount)" | Out-Null
}
function Why($s, $other) { if ((Status $s) -match [regex]::Escape($refused)) { 'refused' } else { $other } }
function Ping($row, $s) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $out = ((P display -p -t $s ping) -join '').Trim()
    $rc = $LASTEXITCODE
    $ms = $sw.ElapsedMilliseconds
    if ($rc -ne 0 -or $out -ne 'ping' -or $ms -ge 1000) { $script:PingFails.Add("${row}/${s} rc=$rc out='$out' ${ms}ms") }
}
function SamePath($a, $b) { "$a".TrimEnd('\', '/') -ieq "$b".TrimEnd('\', '/') }

Write-Host "`n=== shared server: new-session from a hook (#798 step 3) ===" -ForegroundColor Cyan
$spaces = @()
$proc = $null
try {
    $main = NewSpace 'hnsa'
    $spaces += $main
    Use $main
    $dirs = @{}
    foreach ($n in 'dA', 'dB', 'dC', 'dD', 'dE') {
        $d = Join-Path $root $n
        New-Item -ItemType Directory -Force (Join-Path $d 'sub') | Out-Null
        $dirs[$n] = (Resolve-Path $d).Path
    }
    $env:PSMUX_SERVER_DEBUG = '1'
    P new-session -d -s A -x 120 -y 30 -c $dirs['dA'] | Out-Null
    Remove-Item Env:\PSMUX_SERVER_DEBUG -EA SilentlyContinue
    $mainUp = WaitFor { Has A } 8000
    if (-not $mainUp) {
        foreach ($id in 'E1', 'E2', 'E3', 'E4', 'E5', 'E6', 'E10', 'E11', 'E13') { Row $id INFRA 'setup: session A did not start' }
    } else {
        $pa = ServerPid A
        P set -g display-time 60000 | Out-Null

        Fire A 'new-session -d -s FROMHOOK'
        $made = WaitFor { Has FROMHOOK }
        $sm = Status A
        $pf = if ($made) { ServerPid FROMHOOK } else { '' }
        if ($made -and $pf -eq $pa -and $sm -notmatch [regex]::Escape($refused)) { Row E1 PASS 'ok' }
        else { Row E1 FAIL "$(Why A 'timeout') made=$made pid=$pf server=$pa status='$sm'" }
        Ping E1 A
        if ($sm -notmatch [regex]::Escape($refused) -and $sm -notmatch 'created session') { Row E2 PASS 'ok' }
        else { Row E2 FAIL "$(Why A 'shown') status='$sm'" }

        Fire A 'new-session -d -s TWICE \; new-session -d -s TWICE'
        $seen = WaitFor { (Status A) -eq 'duplicate session: TWICE' }
        Start-Sleep -Milliseconds 500
        $sm = Status A
        $count = @(Names | Where-Object { $_ -eq 'TWICE' }).Count
        $logPath = Join-Path $main.Data 'server_debug.log'
        $logHits = 0
        if (Test-Path $logPath) {
            $logHits = @(Get-Content $logPath -EA SilentlyContinue | Where-Object { $_ -match 'duplicate session: TWICE' }).Count
        }
        if ($seen -and $count -eq 1 -and $sm -eq 'duplicate session: TWICE' -and $logHits -ge 1) { Row E3 PASS 'ok' }
        else { Row E3 FAIL "$(Why A 'timeout') count=$count status='$sm' log=$logHits" }
        Ping E3 A

        $before = Names
        Fire A 'new-session -d \; new-session -d'
        $script:Fresh = @()
        $two = WaitFor { $script:Fresh = @(Names | Where-Object { $before -notcontains $_ }); $script:Fresh.Count -ge 2 }
        $pids = @($script:Fresh | ForEach-Object { ServerPid $_ })
        $distinct = @($script:Fresh | Select-Object -Unique).Count
        if ($two -and $script:Fresh.Count -eq 2 -and $distinct -eq 2 -and @($pids | Where-Object { $_ -ne $pa }).Count -eq 0) { Row E4 PASS 'ok' }
        else { Row E4 FAIL "$(Why A 'timeout') new='$($script:Fresh -join ',')' pids='$($pids -join ',')'" }
        Ping E4 A

        foreach ($i in 1..30) { P set -g "@g$i" "v$i" | Out-Null }
        P set -g '@rp' X | Out-Null
        Fire A 'new-session -d -s RP'
        $made = WaitFor { Has RP }
        $script:Seen5 = ''
        $ok = $made -and (WaitFor { $script:Seen5 = ((P display -p -t RP '#{@rp}|#{@g1}|#{@g30}') -join '').Trim(); $script:Seen5 -eq 'X|v1|v30' })
        if ($ok) { Row E5 PASS 'ok' } else { Row E5 FAIL "$(Why A 'timeout') made=$made value='$($script:Seen5)'" }
        Ping E5 A

        $k = 0
        foreach ($n in 'dB', 'dC', 'dD', 'dE') {
            $k++
            P new-session -d -s "W$k" -x 120 -y 30 -c $dirs[$n] | Out-Null
        }
        $k = 0
        $ready = $true
        foreach ($n in 'dB', 'dC', 'dD', 'dE') {
            $k++
            $sp = ((P display -p -t "W$k" '#{session_path}') -join '').Trim()
            if (-not (SamePath $sp $dirs[$n])) { $ready = $false }
        }
        if (-not $ready) { Row E11 INFRA 'setup: the other sessions do not start in their directories' }
        else {
            Fire A 'new-session -d -s RELC -c sub'
            $made = WaitFor { Has RELC }
            $want = Join-Path $dirs['dA'] 'sub'
            $script:Seen11 = ''
            $ok = $made -and (WaitFor { $script:Seen11 = ((P display -p -t RELC '#{pane_current_path}') -join '').Trim(); SamePath $script:Seen11 $want })
            if ($ok) { Row E11 PASS 'ok' } else { Row E11 FAIL "$(Why A 'timeout') made=$made path='$($script:Seen11)' want='$want'" }
            Ping E11 A
        }

        $missing = Join-Path $root 'no-such-directory'
        P new-session -d -s A2 -x 120 -y 30 -c $missing | Out-Null
        if (-not (WaitFor { Has A2 })) { Row E13 INFRA 'setup: session A2 did not start' }
        else {
            Fire A2 'new-session -d -s NOCWD'
            [void](WaitFor { (Status A2) -ne '' })
            Start-Sleep -Milliseconds 300
            $made = Has NOCWD
            $sm = Status A2
            if (-not $made -and $sm -match 'has no start directory') { Row E13 PASS 'ok' }
            else { Row E13 FAIL "$(Why A2 'other') made=$made status='$sm'" }
            Ping E13 A2
        }

        $proc = Start-Process -FilePath $Exe -ArgumentList "-L", $script:Ns, "attach", "-t", "A" -PassThru
        $attached = WaitFor { ((P display -p -t A '#{session_attached}') -join '').Trim() -eq '1' } 8000
        if (-not $attached) {
            Row E6 INFRA 'setup: no client attached to A'
            Row E10 INFRA 'setup: no client attached to A'
        } else {
            Start-Sleep -Milliseconds 800
            Fire A 'new-session -s NOD'
            $made = WaitFor { Has NOD }
            Start-Sleep -Milliseconds 300
            $attA = ((P display -p -t A '#{session_attached}') -join '').Trim()
            $attN = if ($made) { ((P display -p -t NOD '#{session_attached}') -join '').Trim() } else { '' }
            if ($made -and $attA -eq '1' -and $attN -eq '0') { Row E6 PASS 'ok' }
            else { Row E6 FAIL "$(Why A 'timeout') made=$made A=$attA NOD=$attN" }
            Ping E6 A

            P bind -T copy-mode Q 'new-session -d -s FROMCOPY' | Out-Null
            P bind -T copy-mode-vi Q 'new-session -d -s FROMCOPY' | Out-Null
            P copy-mode -t A | Out-Null
            Start-Sleep -Milliseconds 500
            & $injector $proc.Id "Q" | Out-Null
            $made = WaitFor { Has FROMCOPY }
            $pc = if ($made) { ServerPid FROMCOPY } else { '' }
            if ($made -and $pc -eq $pa) { Row E10 PASS 'ok' }
            else { Row E10 FAIL "$(Why A 'timeout') made=$made pid=$pc server=$pa" }
            Ping E10 A
        }
    }

    $keep = NewSpace 'hnsk'
    $spaces += $keep
    Use $keep
    P new-session -d -s S1 -x 80 -y 24 | Out-Null
    if (-not (WaitFor { Has S1 } 8000)) { Row E7 INFRA 'setup: session S1 did not start' }
    else {
        $p7 = ServerPid S1
        $alive = [bool](Get-Process -Id $p7 -EA SilentlyContinue)
        Fire S1 'new-session -d -s KEEP \; kill-session -t S1'
        $script:Gone7 = $false
        $script:Kept7 = $false
        $script:Alive7 = $true
        [void](WaitFor {
            $script:Gone7 = -not (Has S1)
            $script:Kept7 = Has KEEP
            $script:Alive7 = [bool](Get-Process -Id $p7 -EA SilentlyContinue)
            ($script:Gone7 -and $script:Kept7) -or -not $script:Alive7
        } 5000)
        $pk = if ($script:Kept7) { ServerPid KEEP } else { '' }
        if ($script:Gone7 -and $script:Kept7 -and $pk -eq $p7) { Row E7 PASS 'ok'; Ping E7 KEEP }
        elseif (-not $script:Alive7 -and -not $script:Kept7) { Row E7 FAIL "server-gone before=$alive pid=$p7" }
        else { Row E7 FAIL "timeout gone=$($script:Gone7) kept=$($script:Kept7) pid=$pk server=$p7 alive=$($script:Alive7)" }
    }

    $died = NewSpace 'hnsd'
    $spaces += $died
    Use $died
    P new-session -d -s S2 -x 80 -y 24 'ping -n 6 127.0.0.1' | Out-Null
    if (-not (WaitFor { Has S2 } 8000)) { Row E12 INFRA 'setup: session S2 did not start' }
    else {
        $p12 = ServerPid S2
        P set-hook -t S2 pane-died 'new-session -d -s AFTER' | Out-Null
        $deadBefore = ((P display -p -t S2 '#{pane_dead}') -join '').Trim()
        if (-not (Has S2) -or $deadBefore -eq '1') { Row E12 INFRA 'setup: the pane ended before the hook was set' }
        else {
            $script:Gone12 = $false
            [void](WaitFor { $script:Gone12 = -not (Has S2); $script:Gone12 -and (Has AFTER) } 8000)
            $after = Has AFTER
            $pa12 = if ($after) { ServerPid AFTER } else { '' }
            $serverAlive = [bool](Get-Process -Id $p12 -EA SilentlyContinue)
            if ($script:Gone12 -and $after -and $pa12 -eq $p12) { Row E12 PASS 'ok'; Ping E12 AFTER }
            elseif (-not $serverAlive -and -not $after) { Row E12 FAIL "server-gone pid=$p12" }
            else { Row E12 FAIL "timeout gone=$($script:Gone12) after=$after pid=$pa12 server=$p12 alive=$serverAlive" }
        }
    }

    $ren = NewSpace 'hnsr'
    $spaces += $ren
    Use $ren
    P new-session -d -s RA -x 80 -y 24 -c $dirs['dC'] | Out-Null
    P new-session -d -s RB -x 80 -y 24 -c $dirs['dD'] | Out-Null
    if (-not (WaitFor { (Has RA) -and (Has RB) } 8000)) { Row E14 INFRA 'setup: sessions RA and RB did not start' }
    else {
        P set -g display-time 60000 | Out-Null
        $pr = ServerPid RB
        Fire RA 'rename-session RX \; new-session -d -s RENEW -c sub'
        $made = WaitFor { Has RENEW }
        $renamed = (Names) -contains 'RX'
        $want = Join-Path $dirs['dC'] 'sub'
        $script:Seen14 = ''
        $ok = $made -and (WaitFor { $script:Seen14 = ((P display -p -t RENEW '#{pane_current_path}') -join '').Trim(); SamePath $script:Seen14 $want })
        $pn = if ($made) { ServerPid RENEW } else { '' }
        if ($ok -and $renamed -and $pn -eq $pr) { Row E14 PASS 'ok' }
        else { Row E14 FAIL "$(Why RA 'timeout') made=$made renamed=$renamed path='$($script:Seen14)' want='$want' pid=$pn server=$pr" }
        Ping E14 RB
    }

    $req = NewSpace 'hnsq'
    $spaces += $req
    Use $req
    P new-session -d -s QA -x 80 -y 24 -c $dirs['dC'] | Out-Null
    P new-session -d -s QB -x 80 -y 24 -c $dirs['dD'] | Out-Null
    if (-not (WaitFor { (Has QA) -and (Has QB) } 8000)) { Row E15 INFRA 'setup: sessions QA and QB did not start' }
    else {
        P set -g display-time 60000 | Out-Null
        $pq = ServerPid QB
        Fire QA 'new-session -d -s QNEW -c sub \; rename-session QX'
        $made = WaitFor { Has QNEW }
        $renamed = (Names) -contains 'QX'
        $want = Join-Path $dirs['dC'] 'sub'
        $script:Seen15 = ''
        $ok = $made -and (WaitFor { $script:Seen15 = ((P display -p -t QNEW '#{pane_current_path}') -join '').Trim(); SamePath $script:Seen15 $want })
        $pn = if ($made) { ServerPid QNEW } else { '' }
        if ($ok -and $renamed -and $pn -eq $pq) { Row E15 PASS 'ok' }
        else { Row E15 FAIL "$(Why QA 'timeout') made=$made renamed=$renamed path='$($script:Seen15)' want='$want' pid=$pn server=$pq" }
        Ping E15 QB
    }

    if ($script:PingFails.Count -eq 0) { Row E8 PASS 'ok' } else { Row E8 FAIL ($script:PingFails -join '; ') }
} finally {
    foreach ($space in $spaces) {
        Use $space
        P kill-server | Out-Null
    }
    if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue }
    RestoreEnv
    ClearSwitchEnv
    $defaultAfter = DefaultLs
    RestoreEnv
    Remove-Item -Recurse -Force $root -EA SilentlyContinue
}

$lost = @($defaultBefore | Where-Object { $_ -match '^\S+ \d+$' -and $defaultAfter -notcontains $_ })
$leaked = @($defaultAfter | Where-Object { $suiteNames -contains ($_ -split ' ')[0] })
$added = @($defaultAfter | Where-Object { $defaultBefore -notcontains $_ -and $leaked -notcontains $_ })
foreach ($line in $added) { Write-Host "  WARN the default namespace has a new line: $line" -ForegroundColor Yellow }
if ($lost.Count -eq 0 -and $leaked.Count -eq 0) { Row E9 PASS 'ok' }
else { Row E9 FAIL "default lost='$($lost -join ',')' leaked='$($leaked -join ',')'" }

$script:Rows | Set-Content -Path $Result -Encoding utf8
$states = @($script:Rows | ForEach-Object { ($_ -split ' ')[2] })
Write-Host "`n=== Results: $Result ===" -ForegroundColor Cyan
if ($states -contains 'INFRA') { exit 2 }
if ($states -contains 'FAIL') { exit 1 }
exit 0
