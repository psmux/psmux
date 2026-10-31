# Issue #945: codepoint-widths as tmux's array option. Each element keeps its
# own index, so `set -s codepoint-widths[N]` and `set -su codepoint-widths[N]`
# work on one element and `show` prints the indexes as they are; `-a` takes
# the lowest free index; `show -v` prints a value a line and
# `#{codepoint-widths}` joins the elements with a space; and the width table
# is built from the elements in index order, so a later index wins.
#
# Every expected line below is what tmux 3.8 (`-f /dev/null`, WSL) printed
# for the same sequence of commands, except [9] and [11]. [9]: psmux keeps
# an element without the spaces around it, where tmux keeps them and the
# element then sets no width. [11]: see there.
#
# The widths are read from a 20 column pane: 15 U+2502 fit on one row at one
# column each and wrap 10 + 5 at two. Each check opens a new window in the
# same session, since a `set -s` reaches only that session's server.
#
# Isolated: USERPROFILE, PSMUX_DATA_DIR and PSMUX_CONFIG_FILE point into a
# fresh temp dir and every command uses -L. PSMUX_EXE picks the binary
# (defaults to psmux on PATH).

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$TempHome = Join-Path $env:TEMP ("psmux_cpwa_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "empty.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_TARGET*' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
# A spare shell from the warm pool can come up before the window's command
# runs; the widths do not depend on it, but an empty pane would be retried.
$env:PSMUX_NO_WARM = "1"
$NS = "cpwa_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function P { & $PSMUX -L $NS @args 2>&1 }
function Check($name, $got, $want) {
    if ("$got" -eq "$want") { Write-Pass "$name = '$got'" } else { Write-Fail "$name expected '$want', got '$got'" }
}
function Show { (P show -s codepoint-widths) -join ' | ' }

# Clear the screen first so the console host repaints from the top left, then
# draw 15 U+2502 and wait.
$payload = Join-Path $TempHome "bars.ps1"
@'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$e = [char]27
[Console]::Out.Write("$e[H$e[2J")
[Console]::Out.Write(([string][char]0x2502) * 15)
[Console]::Out.Flush()
Start-Sleep -Seconds 60
'@ | Set-Content -Path $payload -Encoding UTF8

# The rows of U+2502 a new window draws, as "15" or "10,5".
$script:window = 0
function Bars {
    for ($try = 1; $try -le 3; $try++) {
        $script:window++
        $w = "b$($script:window)"
        P new-window -t s -n $w "pwsh -NoProfile -NoLogo -File $payload" | Out-Null
        $counts = @()
        for ($i = 0; $i -lt 20 -and $counts.Count -eq 0; $i++) {
            Start-Sleep -Milliseconds 250
            $counts = @(P capture-pane -p -t "s:$w" | ForEach-Object {
                @($_.ToCharArray() | Where-Object { [int]$_ -eq 0x2502 }).Count
            } | Where-Object { $_ -gt 0 })
        }
        # Let the row finish drawing before it is read for good.
        Start-Sleep -Milliseconds 500
        $counts = @(P capture-pane -p -t "s:$w" | ForEach-Object {
            @($_.ToCharArray() | Where-Object { [int]$_ -eq 0x2502 }).Count
        } | Where-Object { $_ -gt 0 })
        P kill-window -t "s:$w" | Out-Null
        if ($counts.Count -gt 0) { return ($counts -join ',') }
    }
    return ''
}

try {
    P new-session -d -s s -x 20 -y 10 | Out-Null
    Start-Sleep -Seconds 2
    P has-session -t s | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Fail "session did not start"; exit 1 }

    Write-Host "`n[0] a new server has no element" -ForegroundColor Yellow
    Check "show -s codepoint-widths" (Show) 'codepoint-widths'
    Check "U+2502 with no override" (Bars) '15'

    Write-Host "`n[1] a value without an index makes one element per entry" -ForegroundColor Yellow
    P set -s codepoint-widths 'U+2502=2,U+4E2D=1' | Out-Null
    Check "show" (Show) 'codepoint-widths[0] U+2502=2 | codepoint-widths[1] U+4E2D=1'
    Check "U+2502 at two columns" (Bars) '10,5'

    Write-Host "`n[2] codepoint-widths[N] sets one element" -ForegroundColor Yellow
    P set -s 'codepoint-widths[5]' 'U+25CF=2' | Out-Null
    Check "show" (Show) 'codepoint-widths[0] U+2502=2 | codepoint-widths[1] U+4E2D=1 | codepoint-widths[5] U+25CF=2'

    Write-Host "`n[3] -a takes the lowest free index" -ForegroundColor Yellow
    P set -sa codepoint-widths 'U+2603=2' | Out-Null
    Check "show" (Show) 'codepoint-widths[0] U+2502=2 | codepoint-widths[1] U+4E2D=1 | codepoint-widths[2] U+2603=2 | codepoint-widths[5] U+25CF=2'

    Write-Host "`n[4] reading it back" -ForegroundColor Yellow
    Check "show -sv" ((P show -sv codepoint-widths) -join ' | ') 'U+2502=2 | U+4E2D=1 | U+2603=2 | U+25CF=2'
    Check "#{codepoint-widths}" ((P display -p '#{codepoint-widths}') -join ' | ') 'U+2502=2 U+4E2D=1 U+2603=2 U+25CF=2'
    Check "show -s codepoint-widths[5]" ((P show -s 'codepoint-widths[5]') -join ' | ') 'codepoint-widths[5] U+25CF=2'
    Check "show -s codepoint-widths[9]" ((P show -s 'codepoint-widths[9]') -join ' | ') "codepoint-widths[9] ''"

    Write-Host "`n[5] -u codepoint-widths[N] removes one, and its width goes" -ForegroundColor Yellow
    P set -su 'codepoint-widths[0]' | Out-Null
    Check "show" (Show) 'codepoint-widths[1] U+4E2D=1 | codepoint-widths[2] U+2603=2 | codepoint-widths[5] U+25CF=2'
    Check "U+2502 back to one column" (Bars) '15'

    Write-Host "`n[6] an element alone sets its width" -ForegroundColor Yellow
    P set -su codepoint-widths | Out-Null
    Check "show after -u" (Show) 'codepoint-widths'
    P set -s 'codepoint-widths[3]' 'U+2502=2' | Out-Null
    Check "show" (Show) 'codepoint-widths[3] U+2502=2'
    Check "U+2502 at two columns" (Bars) '10,5'

    Write-Host "`n[7] a later index wins, whichever was set first" -ForegroundColor Yellow
    P set -s 'codepoint-widths[5]' 'U+2502=1' | Out-Null
    P set -s 'codepoint-widths[0]' 'U+2502=2' | Out-Null
    Check "show" (Show) 'codepoint-widths[0] U+2502=2 | codepoint-widths[3] U+2502=2 | codepoint-widths[5] U+2502=1'
    Check "U+2502 from [5]" (Bars) '15'

    Write-Host "`n[8] -a on an element appends to its text" -ForegroundColor Yellow
    P set -s codepoint-widths 'U+2502=2' | Out-Null
    P set -sa 'codepoint-widths[0]' 'x' | Out-Null
    Check "show" (Show) 'codepoint-widths[0] U+2502=2x'

    Write-Host "`n[9] spaces around an element are trimmed (tmux keeps them)" -ForegroundColor Yellow
    P set -s codepoint-widths 'U+2502=2, U+4E2D=1' | Out-Null
    Check "show" (Show) 'codepoint-widths[0] U+2502=2 | codepoint-widths[1] U+4E2D=1'
    P set -s codepoint-widths ' U+2502=2' | Out-Null
    Check "show" (Show) 'codepoint-widths[0] U+2502=2'
    Check "U+2502 at two columns" (Bars) '10,5'

    Write-Host "`n[10] an empty value and empty pieces" -ForegroundColor Yellow
    P set -s codepoint-widths '' | Out-Null
    Check "show" (Show) 'codepoint-widths'
    P set -s codepoint-widths ',,U+2502=2,,' | Out-Null
    Check "show" (Show) 'codepoint-widths[0] U+2502=2'

    Write-Host "`n[11] a bad index is refused and stores nothing" -ForegroundColor Yellow
    # As for every array since #921, after tmux 3.7c. tmux 3.8 also keys an
    # element by a name (options.c:62-76, options_array_correct_key) and
    # keeps `codepoint-widths[x]`; that applies to every array and is left to
    # a change of its own.
    $out = (P set -s 'codepoint-widths[x]' 'U+2603=2') -join ' | '
    $rc = $LASTEXITCODE
    if ($rc -ne 0 -and $out -match 'invalid option: codepoint-widths\[x\]') { Write-Pass "set codepoint-widths[x] fails: rc=$rc '$out'" } else { Write-Fail "set codepoint-widths[x] rc=$rc '$out'" }
    Check "show after the bad index" (Show) 'codepoint-widths[0] U+2502=2'

    Write-Host "`n[12] a config file uses tmux's spelling" -ForegroundColor Yellow
    $conf = Join-Path $TempHome "cpw.conf"
    Set-Content -Path $conf -Value @("set -su codepoint-widths", "set -s codepoint-widths[4] 'U+2502=2'", "set -sa codepoint-widths 'U+4E2D=1'")
    P source-file $conf | Out-Null
    Check "show" (Show) 'codepoint-widths[0] U+4E2D=1 | codepoint-widths[4] U+2502=2'
    Check "U+2502 at two columns" (Bars) '10,5'
}
finally {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
