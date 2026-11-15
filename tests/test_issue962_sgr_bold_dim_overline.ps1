# Issue #962: bold together with dim (`CSI 1 ; 2 m`), and the overline
# (`CSI 53 m` / `CSI 55 m`), as tmux 3.8 keeps them (input.c:2420-2425,
# :2450-2451, :2497-2501 at tag 3.8). psmux kept only the last of bold and
# dim, and had no overline.
#
# Every expectation is what tmux 3.8 kept for the same bytes (`capture-pane
# -e` in WSL); psmux writes the codes in its own order, a `0` first.
#
# [1] a pane on the default console host (the inbox conhost) shows each
# payload, read back with `capture-pane -e`. [2] the same on OpenConsole
# (PSMUX_CONPTY_DIR), when $env:PSMUX_TEST_CONPTY_DIR holds conpty.dll; skipped otherwise. [3] the frame the server sends a client
# (dump-state) carries the overline and both flags. [4] a real attached
# client draws the overline: its console cell has COMMON_LVB_GRID_HORIZONTAL,
# read with tests\conread.cs.
#
# The inbox conhost of Windows 10 (19045) turns bold into a bright colour and
# drops dim and the overline before psmux sees them, so there [1] expects what
# that host hands psmux, and [3] runs on the OpenConsole server of [2], which
# passes the codes on (skipped without it). [4] is skipped there: the client's
# own console is that host, which draws no overline.
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per server;
# cleanup is `-L <ns> kill-server` plus the client PID. PSMUX_EXE picks the
# binary (defaults to target\release\psmux.exe).

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -and $_.Name -ne 'PSMUX_EXE' -and $_.Name -ne 'PSMUX_TEST_CONPTY_DIR' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
$PSMUX = $env:PSMUX_EXE
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }
$ConptyDir = if ($env:PSMUX_TEST_CONPTY_DIR) { $env:PSMUX_TEST_CONPTY_DIR } else { '' }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkGray }
function Check($name, $got, $want) {
    if ("$got" -eq "$want") { Write-Pass "$name = '$got'" } else { Write-Fail "$name expected '$want', got '$got'" }
}

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_sgr_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""
$E = [char]27
# name, payload, what capture-pane -e row 0 must hold, and what it holds
# through the inbox conhost of Windows 10, which hands psmux bold as bold and
# bright white (or bright red) and no dim or overline
$cases = @(
    @('bold_dim', "a$E[1;2mX$E[m", "$E[0;1;2mX", "$E[0;1;97mX"),
    @('bold_then_dim', "a$E[1m$E[2mX$E[m", "$E[0;1;2mX", "$E[0;1;97mX"),
    @('dim_then_bold', "a$E[2m$E[1mX$E[m", "$E[0;1;2mX", "$E[0;1;97mX"),
    @('sgr22', "a$E[1;2mA$E[22mB$E[m", "$E[0;1;2mA$E[0mB", "$E[0;1;97mA$E[0mB"),
    @('overline', "a$E[53mO$E[55mP$E[m", "$E[0;53mO$E[0mP", "$E[0maOP"),
    @('overline_sgr0', "a$E[53mO$E[0mP$E[m", "$E[0;53mO$E[0mP", "$E[0maOP"),
    @('overline_mix', "a$E[1;53;31mO$E[m", "$E[0;1;53;31mO", "$E[0;1;91mO")
)
# The inbox conhost before Windows 11 (measured on 19045).
$Win10 = [Environment]::OSVersion.Version.Build -lt 22000
$payloadDir = Join-Path $TempHome "p"
New-Item -ItemType Directory -Force $payloadDir | Out-Null
$show = Join-Path $TempHome "show.ps1"
Set-Content -Path $show -Value @(
    'param([string]$Payload)'
    '[Console]::OutputEncoding = [Text.Encoding]::UTF8'
    '[Console]::Out.Write([IO.File]::ReadAllText($Payload))'
    'Start-Sleep 600'
)
foreach ($c in $cases) { [IO.File]::WriteAllText((Join-Path $payloadDir "$($c[0]).txt"), $c[1]) }
$pwsh = (Get-Command pwsh).Source.Replace('\', '/')
$fwd = $TempHome.Replace('\', '/')
$script:NS = ""
function P { & $PSMUX -L $script:NS @args 2>&1 }
function New-Server {
    $script:NS = "sgr_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 100 -y 20 | Out-Null
    Start-Sleep -Milliseconds 2000
}
function Show($name) {
    P new-window -t s -n $name "$pwsh -NoProfile -File $fwd/show.ps1 $fwd/p/$name.txt" | Out-Null
    P set-option -w -t "s:$name" automatic-rename off | Out-Null
    Start-Sleep -Milliseconds 2500
}
function Row0($name) { ((P capture-pane -p -e -t "s:$name") | Select-Object -First 1) }
function Visible($s) { $s -replace $E, '\e' }
function Run-Captures($route) {
    New-Server
    foreach ($c in $cases) {
        Show $c[0]
        $row = Row0 $c[0]
        $want = if ($route -eq 'inbox conhost' -and $Win10) { $c[3] } else { $c[2] }
        if ($row -and $row.Contains($want)) { Write-Pass "${route}: $($c[0]) has '$(Visible $want)'" }
        else { Write-Fail "${route}: $($c[0]) expected '$(Visible $want)' in row 0, got '$(Visible $row)'" }
    }
}

Write-Host "`n=== bold with dim, and the overline ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((& $PSMUX -V) -join ' ')"

# [3] and [4] on the server that is running, with its windows from
# Run-Captures.
function Run-Client {
    Write-Host "`n[3] The frame a client gets carries both flags and the overline" -ForegroundColor Yellow
    $port = [int](Get-Content (Join-Path $TempHome "$($script:NS)__s.port") -Raw).Trim()
    $key = (Get-Content (Join-Path $TempHome "$($script:NS)__s.key") -Raw).Trim()
    foreach ($w in @(@('bold_dim', 'X', 3, $false), @('overline', 'O', 0, $true))) {
        P select-window -t "s:$($w[0])" | Out-Null
        Start-Sleep -Milliseconds 400
        $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", $port)
        $st = $tcp.GetStream(); $st.ReadTimeout = 3000
        $wr = [System.IO.StreamWriter]::new($st); $wr.NewLine = "`n"; $wr.AutoFlush = $true
        $rd = [System.IO.StreamReader]::new($st)
        $wr.WriteLine("AUTH $key"); [void]$rd.ReadLine()
        $wr.WriteLine("dump-state")
        $json = $null; try { $json = $rd.ReadLine() } catch {}
        $tcp.Close()
        $m = [regex]::Match("$json", '\{"text":"' + $w[1] + '","fg":"[^"]*","bg":"[^"]*","flags":(\d+),"width":\d+([^}]*)\}')
        if (-not $m.Success) { Write-Fail "$($w[0]): no run for '$($w[1])' in the frame"; continue }
        Check "$($w[0]): flags & 3 of the '$($w[1])' run" ([int]$m.Groups[1].Value -band 3) $w[2]
        Check "$($w[0]): the '$($w[1])' run carries ol" ($m.Groups[2].Value -match '"ol":true') $w[3]
    }

    Write-Host "`n[4] A real client draws the overline (COMMON_LVB_GRID_HORIZONTAL)" -ForegroundColor Yellow
    if ($Win10) {
        # The client's own console, the Windows 10 inbox conhost, keeps no
        # overline on its cells; [3] has shown what the client is sent.
        Write-Info "skipped: the Windows 10 inbox conhost draws no overline"
        return
    }
    $toolDir = Join-Path $TempHome "tools"
    New-Item -ItemType Directory -Force $toolDir | Out-Null
    $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
    $READ = Join-Path $toolDir "conread.exe"
    & $csc /nologo /optimize "/out:$READ" (Join-Path $PSScriptRoot "conread.cs") 2>&1 | Out-Null
    if (-not (Test-Path $READ)) {
        Write-Fail "conread did not compile"
    } else {
        P select-window -t "s:overline" | Out-Null
        $script:client = Start-Process -FilePath $PSMUX -ArgumentList "-L", $script:NS, "attach", "-t", "s" -PassThru
        Start-Sleep -Seconds 4
        $lines = [string[]](& $READ $script:client.Id -a)
        $line = $lines | Where-Object { $_ -match '\] aOP' } | Select-Object -First 1
        if (-not $line) {
            Write-Fail "no 'aOP' row on the client's console: $(($lines | Select-Object -First 3) -join ' / ')"
        } else {
            # "[attr 7x1,1031x1,7x98] aOP": expand the runs to one value per column.
            $rle = [regex]::Match($line, '^\[attr ([^\]]+)\]').Groups[1].Value
            $attrs = @()
            foreach ($part in $rle.Split(',')) { $v, $n = $part.Split('x'); $attrs += @([int]$v) * [int]$n }
            $grid = 0x0400
            Check "client: 'a' has no overline" (($attrs[0] -band $grid) -ne 0) $false
            Check "client: 'O' has the overline" (($attrs[1] -band $grid) -ne 0) $true
            Check "client: 'P' has no overline" (($attrs[2] -band $grid) -ne 0) $false
        }
        Stop-Process -Id $script:client.Id -Force -EA SilentlyContinue
        $script:client = $null
        Start-Sleep -Milliseconds 700
    }
}

$script:client = $null
try {
    Write-Host "`n[1] capture-pane -e on the inbox conhost" -ForegroundColor Yellow
    Run-Captures 'inbox conhost'
    if (-not $Win10) { Run-Client }

    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500

    Write-Host "`n[2] capture-pane -e on OpenConsole" -ForegroundColor Yellow
    if ($ConptyDir -and (Test-Path (Join-Path $ConptyDir 'conpty.dll'))) {
        $env:PSMUX_CONPTY_DIR = $ConptyDir
        Run-Captures 'OpenConsole'
        if ($Win10) { Run-Client }
        P kill-server | Out-Null
        Remove-Item Env:PSMUX_CONPTY_DIR
    } else {
        Write-Info "skipped: no conpty.dll in $ConptyDir (set PSMUX_TEST_CONPTY_DIR)"
        if ($Win10) { Write-Info "[3] and [4] skipped too: the Windows 10 inbox conhost drops dim and the overline" }
    }
}
finally {
    if ($script:client -and -not $script:client.HasExited) { Stop-Process -Id $script:client.Id -Force -EA SilentlyContinue }
    if ($script:NS) { P kill-server | Out-Null }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
