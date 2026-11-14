# Issue #1020: XTSMGRAPHICS, `CSI ? item ; action ; value S`, is answered by psmux.
#
# tmux answers it itself (input_csi_dispatch_sm_graphics, input.c:2103 at tag
# 3.8): reading, resetting or reading the maximum of the colour registers gets
# `CSI ? 1 ; 0 ; 1024 S`, anything else a failure with the value asked. psmux
# answered nothing, though both console hosts pass the query on to it and its
# DA1 reply on OpenConsole lists sixel.
#
# A pane's answer follows what its programs were told by DA1:
#   1. the inbox console host answers DA1 itself, without sixel, and drops a
#      pane's sixel, so every query here is a failure
#   2. OpenConsole, with PSMUX_CONPTY_DIR_FOR_TEST (a directory with conpty.dll
#      and OpenConsole.exe; skipped without it), passes DA1 on to psmux, whose
#      reply lists sixel, so these are tmux's answers
#
# tests/reply_order_probe.cs runs in the pane with `raw` and records the replies
# as they came.
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per console
# host; cleanup is `-L <ns> kill-server`. PSMUX_TEST_BIN or PSMUX_EXE picks the
# binary (defaults to target\release\psmux.exe).

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { $env:PSMUX_EXE }
$ConptyDir = $env:PSMUX_CONPTY_DIR_FOR_TEST
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
# The panes run pwsh commands; SHELL would make psmux start them in bash.
Remove-Item Env:SHELL -EA SilentlyContinue
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkGray }
function Check($name, $got, $want) {
    if ("$got" -ceq "$want") { Write-Pass "$name = '$got'" } else { Write-Fail "$name expected '$want', got '$got'" }
}

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_gfx_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""

$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -Last 1
$probe = Join-Path $TempHome 'reply_order_probe.exe'
if ($csc) {
    & $csc.FullName /nologo /optimize /out:$probe (Join-Path $PSScriptRoot 'reply_order_probe.cs') 2>&1 | Out-Null
}
if (-not (Test-Path $probe)) {
    Write-Fail "build tests\reply_order_probe.cs (csc.exe unavailable)"
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
    exit $script:TestsFailed
}

$E = '\e'
$script:NS = ""
$script:run = 0
function P { & $PSMUX -L $script:NS @args 2>&1 }

# The inbox conhost of Windows 10 (19045) passes a query on only when it next
# draws, so there each query is followed by text; Windows 11's needs none.
$Win10Flush = if ([Environment]::OSVersion.Version.Build -lt 22000) { '.' } else { '' }

# Run the probe with $queries in a new window; returns the replies as they came.
function Probe($queries) {
    $script:run++
    P new-window -d -t s -n "w$($script:run)" | Out-Null
    Start-Sleep -Milliseconds 2500
    $out = Join-Path $TempHome "r_$($script:NS)_$($script:run).txt"
    P send-keys -t "s:w$($script:run)" "& '$probe' '$out' '$queries$Win10Flush' raw" Enter | Out-Null
    for ($k = 0; $k -lt 60 -and -not (Test-Path $out); $k++) { Start-Sleep -Milliseconds 200 }
    Start-Sleep -Milliseconds 300
    if (Test-Path $out) { (Get-Content $out -Raw).Trim() } else { '(no reply file)' }
}

# $sixel: what DA1 tells this console host's programs.
function Run-Cases($route, $sixel) {
    $script:NS = "gfx_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 120 -y 30 | Out-Null
    Start-Sleep -Milliseconds 2500

    $got = Probe "$E[c"
    if ($sixel) {
        Check "${route}: DA1 is psmux's and lists sixel" $got "$E[?1;2;4c"
    } else {
        Check "${route}: DA1 is the host's" ($got -cmatch '^\\e\[\?[0-9;]+c$') $true
        Check "${route}: and does not list sixel" ($got -cmatch '[?;]4[;c]') $false
    }

    Write-Host "  -- the colour registers" -ForegroundColor DarkCyan
    $ok = if ($sixel) { "$E[?1;0;1024S" } else { "$E[?1;3;0S" }
    Check "${route}: read" (Probe "$E[?1;1;0S") $ok
    Check "${route}: reset" (Probe "$E[?1;2;0S") $ok
    Check "${route}: read the maximum" (Probe "$E[?1;4;0S") $ok
    Check "${route}: read with no value" (Probe "$E[?1;1S") $ok

    Write-Host "  -- failures, the value asked coming back" -ForegroundColor DarkCyan
    Check "${route}: set the colour registers" (Probe "$E[?1;3;256S") "$E[?1;3;256S"
    Check "${route}: read the sixel geometry" (Probe "$E[?2;1;7S") "$E[?2;3;7S"
    Check "${route}: read the ReGIS geometry" (Probe "$E[?3;1;0S") "$E[?3;3;0S"
    Check "${route}: no parameters" (Probe "$E[?S") "$E[?0;3;0S"
    # The inbox console host drops a parameter with a colon before psmux sees it
    # (pipe-pane -o shows nothing for it), so only OpenConsole can answer one.
    if ($sixel) {
        Check "${route}: an item with a colon" (Probe "$E[?1:2;1;0S") "$E[?-1;3;0S"
    }

    Write-Host "  -- order, and no reply" -ForegroundColor DarkCyan
    $got = Probe "$E[?1;1;0S$E[?2;1;0S$E[>q"
    $first = [regex]::Escape("$ok$E[?2;3;0S") + '\\eP>\|'
    Check "${route}: replies in the order asked, XTVERSION after them" ($got -cmatch "^$first") $true
    $got = Probe "$E[?1;1;0;0S$E[>q"
    Check "${route}: four parameters get no reply, XTVERSION still comes" ($got -cmatch '^\\eP>\|') $true

    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

Write-Host "`n=== XTSMGRAPHICS ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((& $PSMUX -V) -join ' ')"

try {
    Write-Host "`n[1] the inbox conhost" -ForegroundColor Yellow
    Run-Cases 'inbox conhost' $false

    Write-Host "`n[2] OpenConsole" -ForegroundColor Yellow
    if ($ConptyDir -and (Test-Path (Join-Path $ConptyDir 'conpty.dll'))) {
        $env:PSMUX_CONPTY_DIR = $ConptyDir
        Run-Cases 'OpenConsole' $true
        Remove-Item Env:PSMUX_CONPTY_DIR
    } else {
        Write-Info "skipped: set PSMUX_CONPTY_DIR_FOR_TEST to a folder holding conpty.dll and OpenConsole.exe"
    }
}
catch {
    Write-Fail "exception: $_"
}
finally {
    if ($script:NS) { P kill-server | Out-Null }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
