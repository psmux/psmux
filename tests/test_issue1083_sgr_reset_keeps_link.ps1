# Issue #1083: SGR 0 inside an OSC 8 hyperlink resets the rendition and keeps the link.
#
# A link is not a graphic rendition: OSC 8 starts and ends it, and RIS drops
# it. tmux keeps it through SGR 0 (input.c:2415-2418 at tag 3.8, from
# 9e14c1f8 "SGR 0 should not end hyperlink"), and so do both console hosts.
# psmux ended the link at every SGR 0, so the rest of the link's text was
# plain. tmux 3.8 still ends it at a bare `CSI m` (input.c:2387-2390); psmux
# keeps it there too, as both console hosts do.
#
#   1. the inbox console host keeps the link itself and redraws the text with
#      its own OSC 8 and no reset, so psmux shows the link on master too; the
#      Windows 10 inbox conhost sends OSC 8 ahead of its text, so there no
#      letter has a link
#   2. OpenConsole, with PSMUX_CONPTY_DIR_FOR_TEST (a directory with conpty.dll
#      and OpenConsole.exe; skipped without it), passes the program's bytes on
#
# tests/reply_order_probe.cs writes the bytes in one write. The links are read
# from the pane's rows in dump-state.
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

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_sgrlink_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
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

# `\e` is ESC to the probe. The letters after an ST are not `a` or `e`, which
# the probe would read as `\a` and `\e`.
$OPEN = '\e]8;;http://x/\e\'
$CLOSE = '\e]8;;\e\'
$script:NS = ""
$script:run = 0
function P { & $PSMUX -L $script:NS @args 2>&1 }

function Get-Dump {
    $port = (Get-Content "$TempHome\$($script:NS)__s.port" -Raw).Trim()
    $key = (Get-Content "$TempHome\$($script:NS)__s.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.ReceiveTimeout = 4000
    $st = $tcp.GetStream()
    $w = [System.IO.StreamWriter]::new($st)
    $r = [System.IO.StreamReader]::new($st)
    $w.Write("AUTH $key`n"); $w.Flush(); $null = $r.ReadLine()
    $w.Write("dump-state`n"); $w.Flush()
    $best = $null
    for ($j = 0; $j -lt 80; $j++) {
        try { $l = $r.ReadLine() } catch { break }
        if ($null -eq $l) { break }
        if ($l -ne "NC" -and $l.Length -gt 100) { $best = $l; break }
    }
    $tcp.Close()
    $best
}

# Write `M<n> ` + $bytes + `Z` in a new window and return, for each letter
# between the marker and `Z`, `L` when the cell carries http://x/ and `.` when
# it carries no link.
function Links($bytes) {
    $script:run++
    $n = $script:run
    $win = "s:w$n"
    P new-window -t s -n "w$n" | Out-Null
    # Type once the shell shows its prompt: on a slow machine it can take
    # longer to start than a fixed wait, and keys sent before may be lost.
    for ($k = 0; $k -lt 100 -and -not ((P capture-pane -p -t $win | Out-String) -match 'PS [^\r\n]*>'); $k++) {
        Start-Sleep -Milliseconds 200
    }
    Start-Sleep -Milliseconds 500
    $out = Join-Path $TempHome "r$n.txt"
    P send-keys -t $win "& '$probe' '$out' 'M$n ${bytes}Z' raw" Enter | Out-Null
    for ($k = 0; $k -lt 150 -and -not (Test-Path $out); $k++) { Start-Sleep -Milliseconds 200 }
    Start-Sleep -Milliseconds 600
    P select-window -t $win | Out-Null
    Start-Sleep -Milliseconds 300
    $dump = Get-Dump
    if (-not $dump) { return '(no dump-state)' }
    $res = ''
    $on = $false
    foreach ($m in [regex]::Matches($dump, '\{"text":"((?:[^"\\]|\\.)*)"[^{}]*?\}')) {
        $t = $m.Groups[1].Value
        $link = if ($m.Value -match '"link":"([^"]*)"') { $Matches[1] } else { '' }
        # The command line typed above shows the marker too, with backslashes.
        if (-not $on -and $t -cmatch "M$n " -and -not $t.Contains('\')) {
            $on = $true
            $t = $t.Substring($t.IndexOf("M$n ") + "M$n ".Length)
        }
        if (-not $on) { continue }
        $end = $t.IndexOf('Z')
        if ($end -ge 0) { $t = $t.Substring(0, $end) }
        foreach ($ch in $t.ToCharArray()) {
            if ($ch -ne ' ') {
                $res += $(if ($link -ceq 'http://x/') { 'L' } elseif ($link) { "($link)" } else { '.' })
            }
        }
        if ($end -ge 0) { return $res }
    }
    '(marker not found)'
}

# What is written after the marker, and the links of its letters.
$Cases = @(
    @('CSI 0 m', "${OPEN}x\e[0my${CLOSE}z", 'LL.'),
    @('CSI m, no parameter', "${OPEN}x\e[my${CLOSE}z", 'LL.'),
    @('CSI ; m', "${OPEN}x\e[;my${CLOSE}z", 'LL.'),
    @('CSI 00 m', "${OPEN}x\e[00my${CLOSE}z", 'LL.'),
    @('CSI 1 ; 0 m', "${OPEN}x\e[1;0my${CLOSE}z", 'LL.'),
    @('CSI 0 ; 1 m', "${OPEN}x\e[0;1my${CLOSE}z", 'LL.'),
    @('bold before the link, then CSI 0 m', "\e[1m${OPEN}x\e[0my${CLOSE}z", 'LL.'),
    @('two resets, then more text', "${OPEN}x\e[31my\e[0mw\e[4mv\e[mu${CLOSE}z", 'LLLLL.'),
    @('CSI 0 m, then OSC 8 ends it', "${OPEN}x\e[0m${CLOSE}yz", 'L..'),
    @('CSI 0 m, then a second link', "${OPEN}x\e[0m\e]8;;http://y/\e\y${CLOSE}z", 'L(http://y/).'),
    @('DECSC in the link, DECRC after it ended', "${OPEN}\e7x\e[0m${CLOSE}\e8y${CLOSE}z", 'L.'),
    @('alternate screen entered in the link', "${OPEN}x\e[?1049h\e[0m${CLOSE}\e[?1049ly${CLOSE}z", 'LL.')
)

function Run-Host($label, [switch]$NoLinks) {
    $script:NS = "sgrlink_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 120 -y 30 | Out-Null
    Start-Sleep -Milliseconds 2500
    foreach ($c in $Cases) {
        $want = $c[2]
        # A host that sends OSC 8 ahead of the text hands psmux the letters with no link.
        if ($NoLinks) { $want = $want -replace '\([^)]*\)', '.' -replace 'L', '.' }
        Check "${label}: $($c[0])" (Links $c[1]) $want
    }
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

Write-Host "=== SGR 0 inside an OSC 8 link keeps the link ===" -ForegroundColor Cyan
Write-Host "  -- the inbox console host" -ForegroundColor DarkCyan
if ([Environment]::OSVersion.Version.Build -lt 22000) {
    # The Windows 10 inbox conhost (build 19045) hands psmux the program's
    # OSC 8 sequences ahead of the text they wrap, the opening and the
    # closing one together, so the link has ended before the first letter
    # and no letter can carry it (test_issue361_osc8_hyperlink.ps1 finds no
    # link there either). The cases still check that the text arrives and
    # that no link turns up.
    Write-Info "Windows 10 inbox conhost: OSC 8 arrives ahead of its text, so every letter is expected without a link"
    Run-Host 'inbox conhost' -NoLinks
} else {
    Run-Host 'inbox conhost'
}

if ($ConptyDir -and (Test-Path (Join-Path $ConptyDir 'conpty.dll'))) {
    Write-Host "  -- OpenConsole ($ConptyDir)" -ForegroundColor DarkCyan
    $env:PSMUX_CONPTY_DIR = $ConptyDir
    Run-Host 'OpenConsole'
    Remove-Item Env:PSMUX_CONPTY_DIR -EA SilentlyContinue
} else {
    Write-Info "OpenConsole skipped: set PSMUX_CONPTY_DIR_FOR_TEST to a directory with conpty.dll and OpenConsole.exe"
}

Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
