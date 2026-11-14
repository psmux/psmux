# Issue #1032: tab stops, HT, ESC H (HTS), CSI Ps g (TBC), CSI Ps Z (CBT) and CSI Ps ` (HPA)
# in a pane, as tmux handles them (input.c:1331, :1420, :1854, :1491 and :1784
# at tag 3.8), with the table #{pane_tabs} lists (format.c:1051). psmux's
# vt100 kept no table: HT went to the next multiple of 8, and HTS, TBC, CBT and
# HPA did nothing, so with OpenConsole (PSMUX_CONPTY_DIR, which hands psmux the
# program's output as written) the cursor ended in the wrong column. The inbox
# conhost handles all of them itself and hands psmux plain cursor moves, so
# that route had the right cursor already and never sees the table: there
# #{pane_tabs} stays the default.
#
# Each case is written as raw bytes by a program in a 40 column pane, which
# then records where the console host left ITS cursor ([Console]::CursorLeft).
# Every expectation is what tmux 3.8 showed for the same bytes (#{cursor_x}
# and #{pane_tabs}), and psmux's cursor has to be where the console host's is,
# except past the last column, where tmux and psmux say 40 and the hosts 39.
# The inbox conhost of Windows 10 (19045) moves the cursor its own way: it
# ignores HPA and wraps an HT from the last column. There psmux's cursor has to
# be that host's, past the margin too, in place of tmux's.
#
# [1] the inbox conhost; [2] OpenConsole, when PSMUX_TEST_CONPTY_DIR or
# PSMUX_CONPTY_DIR_FOR_TEST names a folder holding conpty.dll, skipped
# otherwise.
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per server;
# cleanup is `-L <ns> kill-server`. PSMUX_TEST_BIN or PSMUX_EXE picks the
# binary (defaults to target\release\psmux.exe).

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { $env:PSMUX_EXE }
$ConptyDir = if ($env:PSMUX_TEST_CONPTY_DIR) { $env:PSMUX_TEST_CONPTY_DIR } elseif ($env:PSMUX_CONPTY_DIR_FOR_TEST) { $env:PSMUX_CONPTY_DIR_FOR_TEST } else { '' }
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

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_tab_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force "$TempHome\p" | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""

# The inbox conhost before Windows 11 (measured on 19045).
$Win10 = [Environment]::OSVersion.Version.Build -lt 22000
$E = [string][char]27
$Forty = '0123456789012345678901234567890123456789'
$Default = '8,16,24,32'
# name, the bytes written, tmux 3.8's #{cursor_x} and #{pane_tabs}, and
# whether the console host's cursor is the same column (not past the margin).
$cases = @(
    @('ht',            "`t",                                 8,  $Default, $true),
    @('ht_three',      "`t`t`t",                             24, $Default, $true),
    @('ht_from_37',    "${E}[38G`t",                         39, $Default, $true),
    @('ht_from_39',    "${E}[40G`t",                         39, $Default, $true),
    @('hts_3',         "${E}[4G${E}H",                       3,  '3,8,16,24,32', $true),
    @('hts_3_ht',      "${E}[4G${E}H`r`t",                   3,  '3,8,16,24,32', $true),
    @('hts_0',         "${E}[1G${E}H",                       0,  '0,8,16,24,32', $true),
    @('hts_39',        "${E}[40G${E}H",                      39, '8,16,24,32,39', $true),
    @('hts_past',      "$Forty${E}H",                        40, $Default, $false),
    @('tbc_8',         "${E}[9G${E}[g",                      8,  '16,24,32', $true),
    @('tbc0_16',       "${E}[17G${E}[0g",                    16, '8,24,32', $true),
    @('tbc0_none',     "${E}[6G${E}[0g",                     5,  $Default, $true),
    @('tbc3',          "${E}[3g",                            0,  '', $true),
    @('tbc3_ht',       "${E}[3g`r`t",                        39, '', $true),
    @('tbc2',          "${E}[2g",                            0,  $Default, $true),
    @('tbc1',          "${E}[9G${E}[1g",                     8,  $Default, $true),
    @('cbt_20',        "${E}[21G${E}[Z",                     16, $Default, $true),
    @('cbt_16',        "${E}[17G${E}[Z",                     8,  $Default, $true),
    @('cbt2_20',       "${E}[21G${E}[2Z",                    8,  $Default, $true),
    @('cbt0_20',       "${E}[21G${E}[0Z",                    16, $Default, $true),
    @('cbt9_20',       "${E}[21G${E}[9Z",                    0,  $Default, $true),
    @('cbt_0',         "${E}[1G${E}[Z",                      0,  $Default, $true),
    @('cbt_5',         "${E}[6G${E}[Z",                      0,  $Default, $true),
    @('cbt_39',        "${E}[40G${E}[Z",                     32, $Default, $true),
    @('cbt_past',      "$Forty${E}[Z",                       32, $Default, $true),
    @('hts_cbt',       "${E}[4G${E}H${E}[8G${E}[Z",          3,  '3,8,16,24,32', $true),
    @('tbc3_cbt',      "${E}[3g${E}[21G${E}[Z",              0,  '', $true),
    @('hpa_10',        "${E}[10``",                          9,  $Default, $true),
    @('hpa_none',      "${E}[20G${E}[``",                    0,  $Default, $true),
    @('hpa_0',         "${E}[20G${E}[0``",                   0,  $Default, $true),
    @('hpa_200',       "${E}[200``",                         39, $Default, $true),
    @('hpa_row',       "${E}[5;3H${E}[10``",                 9,  $Default, $true),
    @('ris_hts',       "${E}[4G${E}H${E}c",                  0,  $Default, $true),
    @('alt_exit',      "${E}[?1049h${E}[6G${E}H${E}[?1049l", 0,  '5,8,16,24,32', $true)
)
foreach ($c in $cases) { [IO.File]::WriteAllBytes("$TempHome\p\$($c[0]).bin", [Text.Encoding]::UTF8.GetBytes($c[1])) }
Set-Content -Path "$TempHome\show.ps1" -Value @(
    'param([string]$Payload)'
    '[Console]::OutputEncoding = [Text.Encoding]::UTF8'
    '$o = [Console]::OpenStandardOutput(); $b = [IO.File]::ReadAllBytes($Payload); $o.Write($b, 0, $b.Length); $o.Flush()'
    'Set-Content -Path ($Payload + ".host") -Value ([Console]::CursorLeft)'
    'Start-Sleep 600'
)
$pwsh = (Get-Command pwsh).Source.Replace('\', '/')
$fwd = $TempHome.Replace('\', '/')
$script:NS = ""
function P { & $PSMUX -L $script:NS @args 2>&1 }

function Run-Cases($route, $tablesSeen) {
    Get-ChildItem "$TempHome\p\*.host" -EA SilentlyContinue | Remove-Item -Force
    $script:NS = "tab_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 40 -y 10 | Out-Null
    Start-Sleep -Milliseconds 2000
    foreach ($c in $cases) {
        P new-window -d -t s -n $c[0] "& '$pwsh' -NoProfile -File $fwd/show.ps1 $fwd/p/$($c[0]).bin" | Out-Null
    }
    $deadline = (Get-Date).AddSeconds(60)
    while ((Get-Date) -lt $deadline -and @(Get-ChildItem "$TempHome\p\*.host" -EA SilentlyContinue).Count -lt $cases.Count) {
        Start-Sleep -Milliseconds 250
    }
    Start-Sleep -Milliseconds 800
    foreach ($c in $cases) {
        $name = $c[0]
        $width = "$(P display -p -t "s:$name" '#{pane_width}')"
        if ($width -ne '40') { Write-Fail "${route}: $name pane is $width columns, not 40"; continue }
        $cursor = "$(P display -p -t "s:$name" '#{cursor_x}')"
        $tabs = "$(P display -p -t "s:$name" '#{pane_tabs}')"
        $hostCol = "$((Get-Content "$TempHome\p\$name.bin.host" -EA SilentlyContinue) -join '')"
        if ($route -eq 'inbox conhost' -and $Win10) {
            Check "${route}: $name cursor_x is the console host's" $cursor $hostCol
        } else {
            Check "${route}: $name cursor_x" $cursor $c[2]
            if ($c[4]) { Check "${route}: $name cursor_x is the console host's" $cursor $hostCol }
        }
        # The inbox conhost keeps the table to itself: psmux sees the default.
        $wantTabs = if ($tablesSeen) { $c[3] } else { $Default }
        Check "${route}: $name pane_tabs" $tabs $wantTabs
    }
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

# A change of width puts the default back, and of height alone keeps the table
# (screen.c:371-374). Only OpenConsole hands psmux the HTS to keep.
function Run-Resize($route) {
    $script:NS = "tab_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    [IO.File]::WriteAllBytes("$TempHome\p\resize.bin", [Text.Encoding]::UTF8.GetBytes("${E}[4G${E}H"))
    Remove-Item "$TempHome\p\resize.bin.host" -EA SilentlyContinue
    P new-session -d -s s -x 40 -y 10 "& '$pwsh' -NoProfile -File $fwd/show.ps1 $fwd/p/resize.bin" | Out-Null
    for ($k = 0; $k -lt 60 -and -not (Test-Path "$TempHome\p\resize.bin.host"); $k++) { Start-Sleep -Milliseconds 250 }
    Start-Sleep -Milliseconds 500
    Check "${route}: before a resize" "$(P display -p -t s '#{pane_tabs}')" '3,8,16,24,32'
    P resize-window -t s -y 8 | Out-Null
    Start-Sleep -Milliseconds 800
    Check "${route}: after a change of height" "$(P display -p -t s '#{pane_tabs}')" '3,8,16,24,32'
    P resize-window -t s -x 30 | Out-Null
    Start-Sleep -Milliseconds 800
    Check "${route}: after a change of width" "$(P display -p -t s '#{pane_tabs}')" '8,16,24'
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

# send-keys -R (screen_write_reset, screen-write.c:427) and a respawn
# (screen_reinit, screen.c:127) put the default back.
function Run-Reset($route) {
    foreach ($how in @('send-keys -R', 'respawn-pane -k')) {
        $script:NS = "tab_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
        Remove-Item "$TempHome\p\resize.bin.host" -EA SilentlyContinue
        P new-session -d -s s -x 40 -y 10 "& '$pwsh' -NoProfile -File $fwd/show.ps1 $fwd/p/resize.bin" | Out-Null
        for ($k = 0; $k -lt 60 -and -not (Test-Path "$TempHome\p\resize.bin.host"); $k++) { Start-Sleep -Milliseconds 250 }
        Start-Sleep -Milliseconds 500
        Check "${route}: before $how" "$(P display -p -t s '#{pane_tabs}')" '3,8,16,24,32'
        # The respawned program only waits, so it sets no stop of its own.
        if ($how -eq 'send-keys -R') { P send-keys -R -t s | Out-Null } else { P respawn-pane -k -t s "& '$pwsh' -NoProfile -Command Start-Sleep 600" | Out-Null }
        Start-Sleep -Milliseconds 2500
        Check "${route}: after $how" "$(P display -p -t s '#{pane_tabs}')" $Default
        P kill-server | Out-Null
        Start-Sleep -Milliseconds 500
    }
}

Write-Host "`n=== Tab stops ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((& $PSMUX -V) -join ' ')"

try {
    Write-Host "`n[1] the inbox conhost" -ForegroundColor Yellow
    Run-Cases 'inbox conhost' $false

    Write-Host "`n[2] OpenConsole" -ForegroundColor Yellow
    if ($ConptyDir -and (Test-Path (Join-Path $ConptyDir 'conpty.dll'))) {
        $env:PSMUX_CONPTY_DIR = $ConptyDir
        Run-Cases 'OpenConsole' $true
        Run-Resize 'OpenConsole'
        Run-Reset 'OpenConsole'
        Remove-Item Env:PSMUX_CONPTY_DIR
    } else {
        Write-Info "skipped: set PSMUX_TEST_CONPTY_DIR to a folder holding conpty.dll and OpenConsole.exe"
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
