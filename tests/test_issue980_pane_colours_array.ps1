# Issue #980: pane-colours, a pane's default colour palette (tmux 3.8
# options-table.c:1629-1635: a window and pane array of colours, no default,
# `pane-colors` its other name). psmux took the name and did nothing with it.
#
# Every expectation is what tmux 3.8 (`-f /dev/null`, WSL) printed or drew for
# the same commands:
#   [1] the option: an empty default, the values a colour takes and refuses
#       (tmux prints `bad colour: X`; psmux's CLI puts `psmux: set-option: `
#       before it, as for status-bg), a value split on space and `,`, `-a`,
#       `[N]` and `#{pane-colours}`
#   [2] the three tables: `-gw`, `-w` and `-p` each hold their own array, and
#       a pane uses the first that exists, whole (show -pA)
#   [3] drawing: a cell written with an indexed colour (31, 91, 38;5;1) goes
#       to the client in the palette's colour, read from the dump-state frame
#   [4] OSC 4 queries: an index the pane set nothing for reports the
#       pane-colours colour; an OSC 4 set wins; OSC 104 brings it back
#       (tests\palette_reply_probe.cs in the pane reads the replies)
#   [5] -a on one element sets the colour given, and -o with an index asks
#       about that element, in the pane, window and global tables
#   [6] two sessions of a shared server (PSMUX_SERVER_MODE=shared) each
#       report their own pane-colours colour to OSC 4 queries
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per server;
# cleanup is `-L <ns> kill-server`. PSMUX_EXE picks the binary (defaults to
# target\release\psmux.exe).

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
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
function Check($name, $got, $want) {
    if ("$got" -ceq "$want") { Write-Pass "$name" } else { Write-Fail "$name`n      expected: [$want]`n      got:      [$got]" }
}

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_pc_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""
$script:NS = ""
function P { (& $PSMUX -L $script:NS @args 2>&1 | ForEach-Object { "$_" }) -join "`n" }
function Fresh {
    if ($script:NS) { & $PSMUX -L $script:NS kill-server 2>&1 | Out-Null; Start-Sleep -Milliseconds 400 }
    $script:NS = "pc_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    & $PSMUX -L $script:NS new-session -d -s s -x 100 -y 20 2>&1 | Out-Null
    for ($i = 0; $i -lt 40; $i++) {
        & $PSMUX -L $script:NS has-session -t s 2>$null
        if ($LASTEXITCODE -eq 0) { break }
        Start-Sleep -Milliseconds 250
    }
    Start-Sleep -Milliseconds 1500
}

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$PROBE = Join-Path $TempHome "palette_reply_probe.exe"
& $csc /nologo /optimize "/out:$PROBE" (Join-Path $PSScriptRoot "palette_reply_probe.cs") 2>&1 | Out-Null

# The dump-state frame of the active pane, as a client gets it.
function Frame {
    $port = [int](Get-Content (Join-Path $TempHome "$($script:NS)__s.port") -Raw).Trim()
    $key = (Get-Content (Join-Path $TempHome "$($script:NS)__s.key") -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", $port)
    $st = $tcp.GetStream(); $st.ReadTimeout = 3000
    $wr = [System.IO.StreamWriter]::new($st); $wr.NewLine = "`n"; $wr.AutoFlush = $true
    $rd = [System.IO.StreamReader]::new($st)
    $wr.WriteLine("AUTH $key"); [void]$rd.ReadLine()
    $wr.WriteLine("dump-state")
    $json = $null; try { $json = $rd.ReadLine() } catch {}
    $tcp.Close()
    return "$json"
}
function RunFg($json, $text) {
    $m = [regex]::Match($json, '\{"text":"' + [regex]::Escape($text) + '","fg":"([^"]*)"')
    if ($m.Success) { return $m.Groups[1].Value } else { return "(no run for $text)" }
}

Write-Host "`n=== pane-colours ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((& $PSMUX -V) -join ' ')"

try {
    Write-Host "`n[1] The option" -ForegroundColor Yellow
    Fresh
    Check "show -gw: empty" (P show -gw pane-colours) "pane-colours"
    Check "show -gwv: empty" (P show -gwv pane-colours) ""
    Check "the other name" (P show -gw pane-colors) "pane-colours"
    foreach ($v in @('#00ff00', 'blue', 'colour200', 'brightred', 'default', 'terminal')) {
        Check "[1]=$v is taken" (P set -gw 'pane-colours[1]' $v) ""
        Check "[1]=$v shows" (P show -gw 'pane-colours[1]') "pane-colours[1] $v"
    }
    Check "[1]=9 is refused" (P set -gw 'pane-colours[1]' 9) "psmux: set-option: bad colour: 9"
    Check "[1]=zzz is refused" (P set -gw 'pane-colours[1]' zzz) "psmux: set-option: bad colour: zzz"
    Check "and leaves [1] as it was" (P show -gw 'pane-colours[1]') "pane-colours[1] terminal"
    Check "[300] is taken" (P set -gw 'pane-colours[300]' red) ""
    P set -gwu pane-colours | Out-Null
    P set -gw pane-colours 'red,blue green' | Out-Null
    Check "a value splits on space and ," (P show -gw pane-colours) "pane-colours[0] red`npane-colours[1] blue`npane-colours[2] green"
    P set -gwa pane-colours yellow | Out-Null
    Check "-a adds at the lowest free index" ((P show -gw pane-colours) -split "`n" | Select-Object -Last 1) "pane-colours[3] yellow"
    Check "#{pane-colours}" (P display -p '#{pane-colours}') "red blue green yellow"
    P set -gwu pane-colours | Out-Null
    Check "-gwu empties it" (P show -gw pane-colours) "pane-colours"

    Write-Host "`n[2] The global, window and pane tables" -ForegroundColor Yellow
    P set -gw 'pane-colours[1]' '#00ff00' | Out-Null
    P set -w 'pane-colours[2]' '#0000ff' | Out-Null
    P set -p 'pane-colours[3]' '#ff0000' | Out-Null
    Check "show -gw" (P show -gw pane-colours) "pane-colours[1] #00ff00"
    Check "show -w" (P show -w pane-colours) "pane-colours[2] #0000ff"
    Check "show -p" (P show -p pane-colours) "pane-colours[3] #ff0000"
    Check "show -pA: the pane's own, whole" (P show -pA pane-colours) "pane-colours[3] #ff0000"
    P set -pu pane-colours | Out-Null
    Check "show -pA after -pu: the window's, inherited" (P show -pA pane-colours) "pane-colours[2]* #0000ff"
    P set -wu pane-colours | Out-Null
    Check "show -pA after -wu: the global one, inherited" (P show -pA pane-colours) "pane-colours[1]* #00ff00"

    Write-Host "`n[3] Drawing through the palette" -ForegroundColor Yellow
    Fresh
    P set -gw 'pane-colours[1]' '#00ff00' | Out-Null
    P set -gw 'pane-colours[2]' blue | Out-Null
    P set -gw 'pane-colours[9]' '#123456' | Out-Null
    # The pane program writes the bytes as they are (Write-Host would drop
    # the SGR), then waits.
    $E = [char]27
    $payload = Join-Path $TempHome "sgr.txt"
    [IO.File]::WriteAllText($payload, "Q$E[31mR$E[32mG$E[91mB$E[38;5;1mI$E[33mY$E[m")
    $show = Join-Path $TempHome "show.ps1"
    Set-Content -Path $show -Value @(
        'param([string]$Payload)'
        '$o = [Console]::OpenStandardOutput(); $b = [IO.File]::ReadAllBytes($Payload); $o.Write($b, 0, $b.Length); $o.Flush()'
        'Start-Sleep 600'
    )
    $pwsh = (Get-Command pwsh).Source.Replace('\', '/')
    P new-window -t s -n sgr "$pwsh -NoProfile -File $($show.Replace('\', '/')) $($payload.Replace('\', '/'))" | Out-Null
    Start-Sleep -Milliseconds 3000
    $json = Frame
    Check "31 (index 1) is drawn #00ff00" (RunFg $json 'R') "rgb:0,255,0"
    Check "32 (index 2) is drawn blue (index 4)" (RunFg $json 'G') "idx:4"
    Check "91 (index 9) is drawn #123456" (RunFg $json 'B') "rgb:18,52,86"
    Check "38;5;1 (index 1) is drawn #00ff00" (RunFg $json 'I') "rgb:0,255,0"
    Check "33 (index 3, not set) stays yellow" (RunFg $json 'Y') "idx:3"
    P set -gwu pane-colours | Out-Null
    Start-Sleep -Milliseconds 800
    Check "after -gwu, 31 is red again" (RunFg (Frame) 'R') "idx:1"

    Write-Host "`n[4] OSC 4 queries" -ForegroundColor Yellow
    if (-not (Test-Path $PROBE)) {
        Write-Fail "palette_reply_probe did not compile"
    } else {
        Fresh
        P set -gw 'pane-colours[1]' '#00ff00' | Out-Null
        Start-Sleep -Milliseconds 800
        function Ask($queries, $target = 's') {
            $out = Join-Path $TempHome ("r_" + [guid]::NewGuid().ToString('N').Substring(0, 6) + ".txt")
            P send-keys -t $target "& '$PROBE' '$out' '$queries'" Enter | Out-Null
            for ($i = 0; $i -lt 40 -and -not (Test-Path $out); $i++) { Start-Sleep -Milliseconds 250 }
            Start-Sleep -Milliseconds 300
            return ((Get-Content $out -EA SilentlyContinue) -join ' | ')
        }
        Check "index 1 reports the pane-colours colour" (Ask '\e]4;1;?\e\') "1 rgb:0000/ffff/0000"
        Check "an OSC 4 set wins" (Ask '\e]4;1;rgb:11/22/33\e\\e]4;1;?\e\') "1 rgb:1111/2222/3333"
        Check "OSC 104 brings pane-colours back" (Ask '\e]104;1\e\\e]4;1;?\e\') "1 rgb:0000/ffff/0000"
    }

    Write-Host "`n[5] -a and -o on one element" -ForegroundColor Yellow
    # tmux 3.8: -a on one colour element sets the colour given (options.c:
    # 615-629), and -o with an index asks whether that element exists
    # (cmd-set-option.c:300-318), in each of the three tables.
    Fresh
    foreach ($t in @(@('-p', 'show -p'), @('-w', 'show -w'), @('-gw', 'show -gw'))) {
        $flag = $t[0]
        P set $flag 'pane-colours[1]' red | Out-Null
        P set "$($flag)a" 'pane-colours[1]' blue | Out-Null
        Check "set $($flag)a [1] blue after red is blue" (P show $flag pane-colours) "pane-colours[1] blue"
        Check "set $($flag)o [2], which is not set, is taken" (P set "$($flag)o" 'pane-colours[2]' green) ""
        & $PSMUX -L $script:NS set "$($flag)o" 'pane-colours[1]' green 2>&1 | Out-Null
        Check "set $($flag)o [1], which is set, is refused" $LASTEXITCODE "1"
        Check "and [1] stays blue" (P show $flag pane-colours) "pane-colours[1] blue`npane-colours[2] green"
    }
    # The CLI puts its own prefix before tmux's message, which differs by
    # route, so only the message and the exit code are checked here.
    $refused = P set -pa 'pane-colours[5]' zzz
    $rc = $LASTEXITCODE
    Check "set -pa [5] zzz is refused with bad colour" ("$rc " + ($refused -replace '^psmux: (set-option: )?', '')) "1 bad colour: zzz"

    Write-Host "`n[6] Two sessions of a shared server" -ForegroundColor Yellow
    # Pane ids are unique across a shared server's sessions, and the OSC 4
    # reply colours are one table for the process: each session keeps its
    # own panes' colours there.
    if (-not (Test-Path $PROBE)) {
        Write-Fail "palette_reply_probe did not compile"
    } else {
        & $PSMUX -L $script:NS kill-server 2>&1 | Out-Null
        Start-Sleep -Milliseconds 400
        $script:NS = "pcs_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
        $env:PSMUX_SERVER_MODE = 'shared'
        & $PSMUX -L $script:NS new-session -d -s s1 -x 100 -y 20 2>&1 | Out-Null
        & $PSMUX -L $script:NS new-session -d -s s2 -x 100 -y 20 2>&1 | Out-Null
        Remove-Item Env:PSMUX_SERVER_MODE
        for ($i = 0; $i -lt 40; $i++) {
            & $PSMUX -L $script:NS has-session -t s2 2>$null
            if ($LASTEXITCODE -eq 0) { break }
            Start-Sleep -Milliseconds 250
        }
        Start-Sleep -Milliseconds 1500
        P set -w -t s1: 'pane-colours[1]' '#00ff00' | Out-Null
        P set -w -t s2: 'pane-colours[1]' '#0000ff' | Out-Null
        Start-Sleep -Milliseconds 800
        Check "s1's pane reports s1's colour" (Ask '\e]4;1;?\e\' 's1') "1 rgb:0000/ffff/0000"
        Check "s2's pane reports s2's colour" (Ask '\e]4;1;?\e\' 's2') "1 rgb:0000/0000/ffff"
        Check "s1's pane still reports s1's colour" (Ask '\e]4;1;?\e\' 's1') "1 rgb:0000/ffff/0000"
    }
}
catch {
    Write-Fail "exception: $_"
}
finally {
    if ($script:NS) { & $PSMUX -L $script:NS kill-server 2>&1 | Out-Null }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
