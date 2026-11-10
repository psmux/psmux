param(
    [string]$Exe = $(if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }),
    [string]$Result = $(Join-Path $env:TEMP "psmux_shared_direct_run_rows.txt"),
    [int]$Repeat = 20,
    [int]$ClosedRepeat = 5
)

if ($PSVersionTable.PSVersion.Major -lt 7) { Write-Host "needs PowerShell 7" -ForegroundColor Red; exit 2 }
$ErrorActionPreference = "Continue"
$script:Rows = [System.Collections.Generic.List[string]]::new()
$script:FireCount = 0
$script:ServerPids = @{}
$script:Halt = $false

function Row($id, $state, $kind, $reason) {
    $line = "ROW $id $state $kind $reason"
    $script:Rows.Add($line)
    $color = switch ($state) { 'PASS' { 'Green' } 'FAIL' { 'Red' } 'OBS' { 'Cyan' } default { 'Yellow' } }
    Write-Host "  $line" -ForegroundColor $color
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
function DefaultLs {
    $out = @(& $Exe ls -F '#{session_name} #{session_created}' 2>&1 | ForEach-Object { "$_".Trim() } | Where-Object { $_ })
    $code = $LASTEXITCODE
    if ($code -eq 0 -and @($out | Where-Object { $_ -notmatch ' \d+$' }).Count -eq 0) { return , $out }
    if ($code -ne 0 -and $out.Count -eq 1 -and $out[0] -match '^no server running') { return , @() }
    $null
}
$defaultBefore = DefaultLs

$root = Join-Path $env:TEMP "psmux_direct_$(Get-Random)"
$profileDir = Join-Path $root 'home'
$data = Join-Path $root 'data'
New-Item -ItemType Directory -Force $profileDir, $data | Out-Null
$empty = Join-Path $root 'empty.conf'
'' | Set-Content $empty
$env:USERPROFILE = $profileDir
$env:HOME = $profileDir
$env:PSMUX_PSREADLINE_HISTORY = Join-Path $root 'history.txt'
$env:PSMUX_CONFIG_FILE = $empty
$env:PSMUX_SERVER_MODE = 'shared'
$env:PSMUX_DATA_DIR = $data
$script:Ns = "dirrun$(Get-Random)"

function P { & $Exe -L $script:Ns @args 2>&1 }
function Has($s) { P has-session -t "=$s" | Out-Null; $LASTEXITCODE -eq 0 }
function Status($s) {
    $out = @(P dump-state -t $s)
    $code = $LASTEXITCODE
    $d = ($out -join '').Trim()
    $json = $null
    if ($code -eq 0) { try { $json = ConvertFrom-Json -InputObject $d -NoEnumerate -ErrorAction Stop } catch { $json = $null } }
    if ($json -isnot [System.Management.Automation.PSCustomObject] -or -not $json.PSObject.Properties['layout']) { return "<bad:dump-state $s exit $code>" }
    $message = $json.PSObject.Properties['status_message']
    if (-not $message -or $null -eq $message.Value) { return '' }
    if ($message.Value -isnot [string]) { return "<bad:dump-state $s status_message>" }
    $message.Value
}
function Fmt($t, $f) { ((P display -p -t $t $f) -join '').Trim() }
function Opt($s, $o, [string[]]$scope = @()) {
    $out = @(P show @scope -t $s -v $o)
    $code = $LASTEXITCODE
    $v = ($out -join "`n").Trim()
    if ($v -ceq "psmux: invalid option: $o") { return '<none>' }
    if ($code -ne 0 -and $v -match "can't find window") { return '<no-window>' }
    if ($code -ne 0 -or $out.Count -gt 1 -or $v -match '^psmux:|^ERROR|no server|can''t connect') { return "<bad:$v>" }
    $v
}
function WaitFor([scriptblock]$cond, [int]$ms = 3000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 50 }
    return [bool](& $cond)
}
function NotePid($s) {
    $p = Fmt $s '#{pid}'
    if ($p -notmatch '^\d+$') { return $false }
    $script:ServerPids[[int]$p] = $true
    $true
}
function Arm($cmd) {
    P set-hook -t A after-rename-window $cmd | Out-Null
    P set-hook -a -t A after-rename-window 'set -t A: @fired 1' | Out-Null
}
function Fire([int]$ms = 4000, [int]$settle = 1000) {
    $script:FireCount++
    P set -t A: '@fired' 0 | Out-Null
    if ((Opt A '@fired') -ne '0') { return 'INVALID' }
    P rename-window -t 'A:0' "fire$($script:FireCount)" | Out-Null
    if (-not (WaitFor { (Opt A '@fired') -eq '1' } $ms)) { return 'TIMEOUT' }
    if ($settle) { Start-Sleep -Milliseconds $settle }
    'ok'
}
function Ready([string[]]$opts) {
    foreach ($o in $opts) {
        P set -u -t 'B:' $o | Out-Null
        if (-not (WaitFor { (Opt B $o) -eq '<none>' } 2000)) { return "B $o is not empty: $(Opt B $o)" }
    }
    $sa = Status A
    if ($sa -ne '') { return "A status '$sa'" }
    ''
}
function FreshA {
    if (Has A) {
        P kill-session -t A | Out-Null
        if (-not (WaitFor { -not (Has A) } 3000)) { return $false }
    }
    P new-session -d -s A -x 120 -y 30 | Out-Null
    if (-not (WaitFor { Has A } 8000)) { return $false }
    NotePid A
}
function FreshB {
    if (Has B) {
        P kill-session -t B | Out-Null
        if (-not (WaitFor { -not (Has B) } 3000)) { return $false }
    }
    P new-session -d -s B -x 120 -y 30 | Out-Null
    if (-not (WaitFor { Has B } 8000)) { return $false }
    P new-window -d -t 'B:1' | Out-Null
    $up = WaitFor { (Fmt 'B:1' '#{window_index}') -eq '1' } 3000
    (NotePid B) -and $up
}
function Judge($id, $kind, [string[]]$reads, [bool]$ok, $text) {
    $bad = @($reads | Where-Object { $_ -like '<bad:*' })
    if ($bad.Count -ne 0) { Row $id INVALID $kind "read: $($bad -join ' ')" }
    elseif ($ok) { Row $id PASS $kind 'ok' }
    else { Row $id FAIL $kind $text }
}
function Tally($counts) { ($counts.Keys | Sort-Object | ForEach-Object { "$_=$($counts[$_])" }) -join ',' }
function Repeated($id, $kind, $line, $opt, $want, [scriptblock]$after, [string]$from = 'B', [string[]]$scope = @()) {
    if (-not (FreshB)) { Row $id INVALID $kind 'B did not start'; return }
    Arm $line
    $counts = @{}
    for ($i = 1; $i -le $Repeat; $i++) {
        $why = Ready @($opt)
        if ($why) { Row $id INVALID $kind "iteration ${i}: $why"; return }
        $fired = Fire
        if ($fired -ne 'ok') { Row $id $fired $kind "iteration ${i}: @fired not seen"; return }
        $v = Opt $from $opt $scope
        if ($v -like '<bad:*') { Row $id INVALID $kind "iteration ${i}: $v"; return }
        $counts[$v] = 1 + [int]$counts[$v]
        if ($after) { & $after }
    }
    $text = Tally $counts
    if ($kind -eq 'observe') { Row $id OBS $kind $text; return }
    if ($counts[$want] -eq $Repeat) { Row $id PASS $kind $text } else { Row $id FAIL $kind "want $want x$Repeat, got $text" }
}
function Once($id, $kind, $line, [string[]]$opts) {
    if (-not (FreshB)) { Row $id INVALID $kind 'B did not start'; return $false }
    $why = Ready $opts
    if ($why) { Row $id INVALID $kind $why; return $false }
    Arm $line
    $fired = Fire
    if ($fired -ne 'ok') { Row $id $fired $kind '@fired not seen'; return $false }
    $true
}
function ClosedPort {
    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    $listener.Start()
    $port = $listener.LocalEndpoint.Port
    $listener.Stop()
    $client = [Net.Sockets.TcpClient]::new()
    $open = $false
    try { $open = $client.ConnectAsync('127.0.0.1', $port).Wait(200) } catch { $open = $false } finally { $client.Dispose() }
    if ($open) { 0 } else { $port }
}
function PortText($file) { try { [IO.File]::ReadAllText($file) } catch { '' } }
function Fail($state, $why) { @{ State = $state; Why = $why } }
function ClosedPortOnce($line, [string[]]$opts) {
    if (-not (FreshA)) { return Fail INVALID 'start: A did not start' }
    if (-not (FreshB)) { return Fail INVALID 'start: B did not start' }
    Arm $line
    foreach ($o in $opts) {
        $v = Opt B $o
        if ($v -like '<bad:*') { return Fail INVALID "read: B $o $v" }
        if ($v -ne '<none>') { return Fail INVALID "start: B $o=$v" }
    }
    $fired = Opt A '@fired'
    if ($fired -ne '<none>') { return Fail INVALID "start: A @fired=$fired" }
    $sa = Status A
    if ($sa -ne '') { return Fail INVALID "start: A status '$sa'" }
    $closed = ClosedPort
    if ($closed -eq 0) { return Fail INVALID 'port-open' }
    $file = Join-Path $data "$($script:Ns)__B.port"
    $original = PortText $file
    if ($original.Trim() -notmatch '^\d+$') { return Fail INVALID "port-write: original '$original'" }
    $result = $null
    try {
        [IO.File]::WriteAllText($file, "$closed")
        if (-not (WaitFor { (PortText $file) -ceq $original } 7000)) { return Fail INVALID 'heal-wait' }
        [IO.File]::WriteAllText($file, "$closed")
        if ((PortText $file) -cne "$closed") { return Fail INVALID 'port-write' }
        $fired = Fire 3000 0
        if ($fired -ne 'ok') { return Fail $fired '@fired not seen' }
        if ((PortText $file) -cne "$closed") { return Fail INVALID 'healed' }
        $result = 'fired'
    } finally {
        try { [IO.File]::WriteAllText($file, $original) } catch {}
        if ((PortText $file) -cne $original) {
            $script:Halt = $true
            $result = Fail INVALID 'restore'
        }
    }
    if ($result -is [hashtable]) { return $result }
    $reads = @($opts | ForEach-Object { "$_=$(Opt B $_)" }) + @("A=$(Status A)")
    $bad = @($reads | Where-Object { $_ -like '*<bad:*' })
    if ($bad.Count -ne 0) { return Fail INVALID "read: $($bad -join ' ')" }
    $reads -join ';'
}
function ClosedPortRow($id, $kind, $line, [string[]]$opts, $want) {
    if ($script:Halt) { Row $id INVALID $kind 'not run: a port file was not restored'; return }
    $counts = @{}
    for ($i = 1; $i -le $ClosedRepeat; $i++) {
        $seen = ClosedPortOnce $line $opts
        if ($seen -is [hashtable]) { Row $id $seen.State $kind "iteration ${i}: $($seen.Why)"; return }
        $counts[$seen] = 1 + [int]$counts[$seen]
    }
    $text = Tally $counts
    if ($counts[$want] -eq $ClosedRepeat) { Row $id PASS $kind "got $text" } else { Row $id FAIL $kind "got $text" }
}

Write-Host "`n=== shared server: a hook's set for another session runs there (#798 step 3) ===" -ForegroundColor Cyan
try {
    P new-session -d -s A -x 120 -y 30 | Out-Null
    $up = WaitFor { Has A } 8000
    P new-session -d -s C -x 120 -y 30 | Out-Null
    $up = (WaitFor { Has C } 8000) -and $up
    if (-not $up -or -not (NotePid A)) { Row SETUP INVALID same 'A or C did not start' }
    else {
        P set -g display-time 60000 | Out-Null

        Repeated E1 gate 'set -t B: @x 1 \; set -t B: @x 2' '@x' '2' $null

        if (Once E3 same 'kill-window -t B:9 \; set -t B: @x 1' @('@x')) {
            $sb = Status B; $sa = Status A; $v = Opt B '@x'
            Judge E3 same @($sb, $sa, $v) ($sb -ieq "can't find window: 9" -and $sa -eq '' -and $v -eq '1') "B='$sb' A='$sa' @x=$v"
        }

        if (Once E4 same 'set -g -t B: @g 1' @()) {
            P new-session -d -s D -x 120 -y 30 | Out-Null
            if (-not (WaitFor { Has D } 8000)) { Row E4 INVALID same 'D did not start' }
            else {
                $script:Seen4 = ''
                $ok = WaitFor { $script:Seen4 = (@('A', 'B', 'C', 'D') | ForEach-Object { Fmt $_ '#{@g}' }) -join ','; $script:Seen4 -eq '1,1,1,1' } 3000
                if ($ok) { Row E4 PASS same 'ok' } else { Row E4 TIMEOUT same "values='$($script:Seen4)'" }
            }
            P kill-session -t D | Out-Null
        }

        if (FreshB) {
            P set-buffer -b k hello | Out-Null
            $why = Ready @()
            if ($why) { Row E5 INVALID same $why }
            else {
                Arm 'paste-buffer -b k -t B:0.0'
                $fired = Fire
                if ($fired -ne 'ok') { Row E5 $fired same '@fired not seen' }
                else {
                    $pasted = WaitFor { ((P capture-pane -p -t 'B:0.0') -join "`n") -match 'hello' } 5000
                    $kept = ((P show-buffer -b k) -join '').Trim()
                    if (-not $pasted) { Row E5 TIMEOUT same "no hello on B:0.0 in 5000 ms, buffer='$kept'" }
                    elseif ($kept -eq 'hello') { Row E5 PASS same 'ok' }
                    else { Row E5 FAIL same "buffer='$kept'" }
                }
            }
        } else { Row E5 INVALID same 'B did not start' }

        if (Once E6 same 'new-window -d -t B:5' @()) {
            $made = WaitFor { (Fmt 'B:5' '#{window_index}') -eq '5' } 3000
            if ($made) { Row E6 PASS same 'ok' } else { Row E6 TIMEOUT same 'no window 5 on B in 3000 ms' }
        }

        if (Once E7 same "set -t B @s1 v \; set -t 'B*': @s2 v \; set -t =B: @s3 v" @('@s1', '@s2', '@s3')) {
            $reads = @(@('@s1', '@s2', '@s3') | ForEach-Object { Opt B $_ })
            Judge E7 same $reads (($reads -join ',') -eq 'v,v,v') "values=$($reads -join ',')"
        }

        if (FreshB) {
            P set-hook -t B after-select-window 'set -t B: @sel 1' | Out-Null
            P select-window -t 'B:0' | Out-Null
            $why = Ready @('@sel')
            if ($why) { Row E8 INVALID same $why }
            else {
                Arm 'select-window -t B:1'
                $fired = Fire
                if ($fired -ne 'ok') { Row E8 $fired same '@fired not seen' }
                else {
                    $w = Fmt 'B' '#{window_index}'; $s = Opt B '@sel'
                    Judge E8 same @($s) ($w -eq '1' -and $s -eq '1') "window=$w @sel=$s"
                }
            }
        } else { Row E8 INVALID same 'B did not start' }

        Repeated E9 observe 'new-window -d -t B:5 \; set -w -t B:5 @w 1' '@w' $null {
            P kill-window -t 'B:5' | Out-Null
            WaitFor { (Fmt 'B:5' '#{window_index}') -ne '5' } 2000 | Out-Null
        } 'B:5' @('-w')

        Repeated E10 gate 'set -t B: @x 1 \; set -u -t B: @x' '@x' '<none>' $null
        if (FreshB) {
            $why = Ready @('@x')
            P set -t 'B:' '@x' 1 | Out-Null
            $start = Opt B '@x'
            if ($why -or $start -ne '1') { Row E10b INVALID gate "start: $why @x=$start" }
            else {
                Arm 'set -u -t B: @x'
                $fired = Fire
                if ($fired -ne 'ok') { Row E10b $fired gate '@fired not seen' }
                else { $v = Opt B '@x'; Judge E10b gate @($v) ($v -eq '<none>') "@x=$v" }
            }
        } else { Row E10b INVALID gate 'B did not start' }

        if (FreshB) {
            P split-window -d -t 'B:0' | Out-Null
            $two = WaitFor { (Fmt 'B:0' '#{window_panes}') -eq '2' } 3000
            P select-pane -t 'B:0.0' | Out-Null
            $why = Ready @()
            if (-not $two -or $why) { Row E11 INVALID same "panes=$two $why" }
            else {
                Arm 'select-pane -t B:0.1'
                $fired = Fire
                if ($fired -ne 'ok') { Row E11 $fired same '@fired not seen' }
                else { $p = Fmt 'B:0' '#{pane_index}'; if ($p -eq '1') { Row E11 PASS same 'ok' } else { Row E11 FAIL same "active pane=$p" } }
            }
        } else { Row E11 INVALID same 'B did not start' }

        if (Once E12 same 'copy-mode -t B:1.0' @()) {
            $m1 = WaitFor { (Fmt 'B:1.0' '#{pane_in_mode}') -eq '1' } 2000
            $m0 = Fmt 'B:0.0' '#{pane_in_mode}'
            if (-not $m1) { Row E12 TIMEOUT same "B:1.0 not in a mode in 2000 ms, B:0.0 mode=$m0" }
            elseif ($m0 -eq '0') { Row E12 PASS same 'ok' }
            else { Row E12 FAIL same "B:0.0 mode=$m0" }
        }

        Repeated E13 observe 'set -t B: @x "a b" \; set -t B: @x c' '@x' $null $null

        if (Once E14a same 'set -t B: @u a"b"c' @('@u')) {
            $v = Opt B '@u'; $sb = Status B; $sa = Status A
            Judge E14a same @($v, $sb, $sa) ($v -eq 'abc' -and $sb -eq '' -and $sa -eq '') "@u=$v B='$sb' A='$sa'"
        }
        if (Once E14b same "set`t-t B: @v 1" @('@v')) {
            $v = Opt B '@v'; $sb = Status B; $sa = Status A
            Judge E14b same @($v, $sb, $sa) ($v -eq '1' -and $sb -eq '' -and $sa -eq '') "@v=$v B='$sb' A='$sa'"
        }
        if (Once E14c same 'set -t B:9 @y 1' @('@y')) {
            $v = Opt B '@y'; $sb = Status B; $sa = Status A
            Judge E14c same @($v, $sb, $sa) ($v -eq '<none>' -and $sb -ceq "Can't find window: 9" -and $sa -eq '') "@y=$v B='$sb' A='$sa'"
        }
        if (Once E14d same 'set -t B: @plugin ppm' @('@plugin')) {
            $v = Opt B '@plugin'; $sb = Status B; $sa = Status A
            Judge E14d same @($v, $sb, $sa) ($v -eq 'ppm' -and $sb -eq '' -and $sa -eq '') "@plugin=$v B='$sb' A='$sa'"
        }

        ClosedPortRow E2 gate 'set -t B: @x 1 \; set -t B: @x 2' @('@x') '@x=2;A='
        ClosedPortRow E15 distinct 'set -t B: @y "a b" \; set -t B: @x 1' @('@y', '@x') "@y=<none>;@x=<none>;A=can't connect to session: B"
    }
} finally {
    P kill-server | Out-Null
    $script:Left = @()
    $gone = WaitFor { $script:Left = @($script:ServerPids.Keys | Where-Object { Get-Process -Id $_ -EA SilentlyContinue }); $script:Left.Count -eq 0 } 5000
    RestoreEnv
    ClearSwitchEnv
    $defaultAfter = DefaultLs
    RestoreEnv
    Remove-Item -Recurse -Force $root -EA SilentlyContinue
}

if ($script:ServerPids.Count -eq 0) { Row LEFT INVALID same 'no server pid was read' }
elseif ($gone) { Row LEFT PASS same "server pids=$($script:ServerPids.Keys -join ',')" }
else { Row LEFT FAIL same "still running: $($script:Left -join ',')" }
if ($null -eq $defaultBefore -or $null -eq $defaultAfter) { Row DEFAULT INVALID same "the default ls could not be read: before=$($null -ne $defaultBefore) after=$($null -ne $defaultAfter)" }
else {
    $lost = @($defaultBefore | Where-Object { $defaultAfter -cnotcontains $_ })
    $added = @($defaultAfter | Where-Object { $defaultBefore -cnotcontains $_ })
    $used = @($defaultAfter | Where-Object { @('A', 'B', 'C', 'D') -contains ($_ -replace ' \d+$', '') })
    if ($lost.Count -eq 0 -and $used.Count -eq 0) { Row DEFAULT PASS same "ok, other new sessions='$($added -join ',')'" }
    else { Row DEFAULT FAIL same "lost='$($lost -join ',')' test names='$($used -join ',')'" }
}

$script:Rows | Set-Content -Path $Result -Encoding utf8
$states = @($script:Rows | ForEach-Object { ($_ -split ' ')[2] })
Write-Host "`n=== Results: $Result ===" -ForegroundColor Cyan
if ($states -contains 'INVALID' -or $states -contains 'TIMEOUT') { exit 2 }
if ($states -contains 'FAIL') { exit 1 }
exit 0
