# Issue #986 follow up: user keys were stored as U+F1000 to U+F13E8, which is
# also where Nerd Fonts 3 puts Material Design icons (U+F0001 to U+F1AF0), and
# the mouse keys right below them. They now sit in Supplementary Private Use
# Area-B (src/key_base.rs). A glyph from the Nerd Fonts range that the user
# types must still reach the pane, and must not run a binding of User0 (whose
# stored form U+F1000 used to be).
#
# The console route (Windows Terminal, a console window) delivers an IME commit,
# a paste or a Win+. emoji pick of a character outside the BMP as a key down
# and key up record per UTF-16 unit, all in one write; tests\injector.cs
# {UTF16:...} writes exactly that into an attached client's console. The pane
# runs pwsh, and PSReadLine's edit line is read back with capture-pane.
#
# Expectations: the glyphs U+F1000 and U+F1234 arrive, with and without
# user-keys set, as they do on master. The PR as first written dropped them in
# the client as unbound user keys. send-keys of the glyph arrives too. With
# `bind -n User0` set, typing U+F1000 runs no binding: while the keys sat in
# the Nerd Fonts range it ran the binding and the glyph was lost.
#
# Each session is its own server, so the config goes to both `sk` and `con`
# with -f; given to `sk` alone, `con` (where the glyphs are typed) had no
# user-keys and no binding at all.
#
# Isolation: private USERPROFILE and PSMUX_DATA_DIR, a -L namespace, cleanup
# by `-L <ns> kill-server` and by the PIDs this script started.
# Binary: $env:PSMUX_EXE, else ..\target\release\psmux.exe.

$ErrorActionPreference = "Continue"
# capture-pane's text is read as UTF-8 whatever the console's code page is.
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -and $_.Name -ne 'PSMUX_EXE' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
Remove-Item Env:\SHELL -EA SilentlyContinue
$PSMUX = $env:PSMUX_EXE
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

$script:TestsPassed = 0
$script:TestsFailed = 0
$script:OpenedPids = @()
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Check($name, $got, $want) {
    if ("$got" -ceq "$want") { Write-Pass "$name" } else { Write-Fail "$name`n      expected: [$want]`n      got:      [$got]" }
}

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_ukg_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"

$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
$INJ = Join-Path $TempHome "injector.exe"
if ($csc) { & $csc /nologo /optimize /out:$INJ (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null }

$g1 = [char]::ConvertFromUtf32(0xF1000)
$g2 = [char]::ConvertFromUtf32(0xF1234)
$want = "U+41 U+F1000 U+F1234 U+42"

# The code points PSReadLine shows after the prompt of session $s.
function Edit-Line($ns, $s) {
    $text = & $PSMUX -L $ns capture-pane -p -t $s 2>&1 | Out-String
    $line = ($text -split "`n" | Where-Object { $_ -match 'PS .*>' } | Select-Object -Last 1)
    if (-not $line) { return "(no prompt)" }
    $l = $line.Substring($line.IndexOf('>') + 1).Trim()
    (($l.EnumerateRunes() | ForEach-Object { 'U+{0:X}' -f $_.Value }) -join ' ')
}
function Wait-Prompt($ns, $s) {
    for ($i = 0; $i -lt 60; $i++) {
        if ((& $PSMUX -L $ns capture-pane -p -t $s 2>&1 | Out-String) -match 'PS .*>') { return $true }
        Start-Sleep -Milliseconds 500
    }
    return $false
}
function Wait-Line($ns, $s, $want) {
    $got = ""
    for ($i = 0; $i -lt 20; $i++) { $got = Edit-Line $ns $s; if ($got -ceq $want) { break }; Start-Sleep -Milliseconds 300 }
    return $got
}

# One server per case: $conf lines, a pwsh pane, then the glyphs typed into an
# attached console route client.
function Run-Case($label, $confLines) {
    $ns = "ukg" + [guid]::NewGuid().ToString('N').Substring(0, 8)
    $conf = Join-Path $TempHome "$ns.conf"
    Set-Content -Path $conf -Encoding ascii -Value (@('set -g status-left ""') + $confLines)
    & $PSMUX -L $ns -f $conf new-session -d -s sk "pwsh -NoLogo -NoProfile" 2>&1 | Out-Null
    & $PSMUX -L $ns -f $conf new-session -d -s con "pwsh -NoLogo -NoProfile" 2>&1 | Out-Null
    if (-not ((Wait-Prompt $ns "sk") -and (Wait-Prompt $ns "con"))) {
        Write-Fail "$label : the pane shells never showed a prompt"
        & $PSMUX -L $ns kill-server 2>&1 | Out-Null
        return
    }
    Start-Sleep 1

    & $PSMUX -L $ns send-keys -t sk "A" 2>&1 | Out-Null
    & $PSMUX -L $ns send-keys -t sk $g1 2>&1 | Out-Null
    & $PSMUX -L $ns send-keys -t sk -l $g2 2>&1 | Out-Null
    & $PSMUX -L $ns send-keys -t sk "B" 2>&1 | Out-Null
    Check "$label : send-keys of the glyphs reaches the pane" (Wait-Line $ns "sk" $want) $want

    if (-not (Test-Path $INJ)) {
        Write-Fail "$label : could not build tests\injector.cs (csc.exe unavailable)"
    } else {
        $c = Start-Process -FilePath $PSMUX -ArgumentList @("-L", $ns, "attach", "-t", "con") -WindowStyle Minimized -PassThru
        $script:OpenedPids += $c.Id
        Start-Sleep 5
        $u16 = (("$g1$g2").ToCharArray() | ForEach-Object { ([int]$_).ToString("X4") }) -join ","
        Start-Process -FilePath $INJ -ArgumentList @("$($c.Id)", "A{UTF16:$u16}B") -WindowStyle Hidden -Wait | Out-Null
        Check "$label : the glyphs typed on the console route reach the pane" (Wait-Line $ns "con" $want) $want
        Check "$label : the typed glyphs run no binding" ((& $PSMUX -L $ns show -t con -gqv '@hit' 2>&1) -join '') ""
        try { Stop-Process -Id $c.Id -Force -EA SilentlyContinue } catch {}
    }
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
}

Write-Host "[1] user-keys empty (everyone who never sets it)"
Run-Case "no user-keys" @()
Write-Host "[2] user-keys set and User0 bound: the console route still has no user keys"
Run-Case "user-keys set" @('set -s user-keys[0] "\e[5;30012~"', 'bind -n User0 set -g @hit yes')

foreach ($id in $script:OpenedPids) { try { Stop-Process -Id $id -Force -EA SilentlyContinue } catch {} }
Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
Write-Host "=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ==="
exit $script:TestsFailed
