# Issue #991: the mouse keys and the user keys are characters in a private use
# area, and they sat inside the Nerd Fonts 3 Material Design icons (U+F0001 to
# U+F1AF0): the mouse keys from U+F0000, the user keys from U+F1000. A glyph
# from there, typed, pasted or picked, is a key event for that character, so a
# binding of the key it stood for ran and the glyph never reached the pane
# (`bind -n WheelUpPane ...` and U+F08C0, `bind -n User0 ...` and U+F1000).
# The keys now start at U+10E000 in Supplementary Private Use Area-B
# (src/key_base.rs).
#
#   [1] the console route: Nerd Font glyphs at the old places of User0,
#       WheelUpPane and MouseDown1Pane reach the pane and run no binding
#   [2] the VT route: the keys still work at their new place, a user-keys
#       sequence runs `bind -n User0` and a wheel over the pane runs
#       `bind -n WheelUpPane`, and list-keys names them
#   [3] typed on the console route, the old place of User0 (U+F1000) is text
#       and the new one (U+10EC80) is the key, the limit the docs give
#
# The console client is attached in a window of its own and typed into with
# tests\injector.cs; the VT client runs under tests\conpty_vt_keys.cs with
# PSMUX_VT_INPUT=1. Private USERPROFILE and PSMUX_DATA_DIR, a -L namespace
# per server; cleanup is `-L <ns> kill-server` plus the PIDs this suite
# started. PSMUX_EXE picks the binary (defaults to target\release\psmux.exe).

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
$script:OpenedPids = @()
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Check($name, $got, $want) {
    if ("$got" -ceq "$want") { Write-Pass "$name" } else { Write-Fail "$name`n      expected: [$want]`n      got:      [$got]" }
}

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_kb_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"

$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
$INJ = Join-Path $TempHome "injector.exe"
$VTHOST = Join-Path $TempHome "conpty_vt_keys.exe"
if ($csc) {
    & $csc /nologo /optimize "/out:$INJ" (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
    & $csc /nologo /optimize "/out:$VTHOST" (Join-Path $PSScriptRoot "conpty_vt_keys.cs") 2>&1 | Out-Null
}
$POWERSHELL = (Get-Command pwsh -EA SilentlyContinue).Source
if (-not $POWERSHELL) { $POWERSHELL = (Get-Command powershell).Source }

function Conf($lines) {
    $path = Join-Path $TempHome ("c_" + [guid]::NewGuid().ToString('N').Substring(0, 6) + ".conf")
    ((@('set -g status-left ""', 'set -g mouse on', "set -g default-shell '$POWERSHELL -NoLogo -NoProfile'") + $lines) -join "`n") + "`n" | Set-Content $path -Encoding ASCII
    return $path
}
function NewNs { "kb" + [guid]::NewGuid().ToString('N').Substring(0, 6) }
function Opt($ns, $name) { ((& $PSMUX -L $ns show -gqv $name 2>&1) -join '') }
# The code points PSReadLine shows after the prompt.
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
function U16([int]$cp) { (([char]::ConvertFromUtf32($cp).ToCharArray() | ForEach-Object { ([int]$_).ToString("X4") }) -join ",") }
function Hex([string]$s) { ([Text.Encoding]::UTF8.GetBytes($s) | ForEach-Object { '{0:x2}' -f $_ }) -join ',' }
$E = [char]27
$SEQ0 = "$E[5;30012~"
# A wheel up at column 10, row 5 of the pane, as an SGR mouse report.
$WHEEL = "$E[<64;10;5M"

# A VT client under a pseudoconsole: up in 8 s, then $steps, then held open.
$clientVars = @("PSMUX_SESSION", "PSMUX_SESSION_NAME", "PSMUX_PANE", "PSMUX_TARGET_SESSION",
                "TMUX", "TMUX_PANE", "PSMUX_VT_INPUT", "TERM_PROGRAM", "WEZTERM_PANE", "TERMINAL_EMULATOR", "ALACRITTY_LOG")
function Start-VtClient($ns, $conf, $steps) {
    $saved = @{}
    foreach ($k in $clientVars) { $saved[$k] = [Environment]::GetEnvironmentVariable($k); Remove-Item "Env:\$k" -EA SilentlyContinue }
    Set-Item "Env:\PSMUX_VT_INPUT" "1"
    $p = Start-Process -FilePath $VTHOST -WindowStyle Hidden -PassThru `
        -ArgumentList @("w8000,$steps,h4000", "`"$PSMUX`"", "-L", $ns, "-f", "`"$conf`"", "new-session", "-s", "s")
    $script:OpenedPids += $p.Id
    foreach ($k in $clientVars) { if ($null -ne $saved[$k]) { Set-Item "Env:\$k" $saved[$k] } else { Remove-Item "Env:\$k" -EA SilentlyContinue } }
    return $p
}

# [2]: the keys run at their place.
function Keys-Run($label) {
    $ns = NewNs
    $conf = Conf @("set -s user-keys[0] `"\e[5;30012~`"", 'bind -n User0 set -g @hu yes', 'bind -n WheelUpPane set -g @hw yes')
    $p = Start-VtClient $ns $conf "$(Hex $SEQ0),w800,$(Hex $WHEEL)"
    Start-Sleep -Milliseconds 11000
    Check "$label : the user-keys sequence runs bind -n User0" (Opt $ns '@hu') "yes"
    Check "$label : a wheel over the pane runs bind -n WheelUpPane" (Opt $ns '@hw') "yes"
    $root = ((& $PSMUX -L $ns list-keys -T root 2>&1) -join "`n")
    Check "$label : list-keys names User0 and WheelUpPane" ([bool]($root -match '-T root +User0 ' -and $root -match '-T root +WheelUpPane +set')) "True"
    $p | Wait-Process -Timeout 15 -EA SilentlyContinue
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
}

Write-Host "`n=== key base ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((& $PSMUX -V) -join ' ')"

try {
    if (-not (Test-Path $INJ) -or -not (Test-Path $VTHOST)) {
        Write-Fail "could not build tests\injector.cs or tests\conpty_vt_keys.cs (csc.exe unavailable)"
    } else {
        Write-Host "`n[1] Nerd Font glyphs at the old places of the keys are text" -ForegroundColor Yellow
        $ns = NewNs
        # U+F1000 was User0, U+F08C0 WheelUpPane and U+F0014 MouseDown1Pane.
        $conf = Conf @("set -s user-keys[0] `"\e[5;30012~`"", 'bind -n User0 set -g @hu yes',
                       'bind -n WheelUpPane set -g @hw yes', 'bind -n MouseDown1Pane set -g @hm yes')
        & $PSMUX -L $ns -f $conf new-session -d -s con 2>&1 | Out-Null
        if (-not (Wait-Prompt $ns "con")) {
            Write-Fail "the pane shell never showed a prompt"
        } else {
            Start-Sleep 1
            $c = Start-Process -FilePath $PSMUX -ArgumentList @("-L", $ns, "attach", "-t", "con") -WindowStyle Minimized -PassThru
            $script:OpenedPids += $c.Id
            Start-Sleep 5
            # One write per glyph, as an IME commit or an emoji pick delivers it.
            $keys = "A{UTF16:$(U16 0xF1000)}{UTF16:$(U16 0xF08C0)}{UTF16:$(U16 0xF0014)}B"
            Start-Process -FilePath $INJ -ArgumentList @("$($c.Id)", $keys) -WindowStyle Hidden -Wait | Out-Null
            $want = "U+41 U+F1000 U+F08C0 U+F0014 U+42"
            $got = ""
            for ($i = 0; $i -lt 20; $i++) { $got = Edit-Line $ns "con"; if ($got -ceq $want) { break }; Start-Sleep -Milliseconds 300 }
            Check "the three glyphs reach the pane" $got $want
            Check "U+F1000 runs no bind -n User0" (Opt $ns '@hu') ""
            Check "U+F08C0 runs no bind -n WheelUpPane" (Opt $ns '@hw') ""
            Check "U+F0014 runs no bind -n MouseDown1Pane" (Opt $ns '@hm') ""
            try { Stop-Process -Id $c.Id -Force -EA SilentlyContinue } catch {}
        }
        & $PSMUX -L $ns kill-server 2>&1 | Out-Null

        Write-Host "`n[2] The keys still work at their new place (VT route)" -ForegroundColor Yellow
        Keys-Run "new place"

        Write-Host "`n[3] Typed on the console route, the old place is text and the new one the key" -ForegroundColor Yellow
        # The old place of User0 (U+F1000) is a Nerd Font glyph and reaches the
        # pane; the new one (U+10E000 + 3200 = U+10EC80) is the key, so it runs
        # the binding, the limit the docs give.
        $ns = NewNs
        $conf = Conf @('bind -n User0 set -g @hu yes')
        & $PSMUX -L $ns -f $conf new-session -d -s con 2>&1 | Out-Null
        if (Wait-Prompt $ns "con") {
            Start-Sleep 1
            $c = Start-Process -FilePath $PSMUX -ArgumentList @("-L", $ns, "attach", "-t", "con") -WindowStyle Minimized -PassThru
            $script:OpenedPids += $c.Id
            Start-Sleep 5
            Start-Process -FilePath $INJ -ArgumentList @("$($c.Id)", "A{UTF16:$(U16 0xF1000)}B") -WindowStyle Hidden -Wait | Out-Null
            Start-Sleep 2
            Check "the old place of User0 (U+F1000) is text" (Edit-Line $ns "con") "U+41 U+F1000 U+42"
            Check "and runs no binding" (Opt $ns '@hu') ""
            # Cleared, so the check below sees only what U+10EC80 does.
            & $PSMUX -L $ns set -gu '@hu' 2>&1 | Out-Null
            Start-Process -FilePath $INJ -ArgumentList @("$($c.Id)", "{UTF16:$(U16 0x10EC80)}") -WindowStyle Hidden -Wait | Out-Null
            Start-Sleep 2
            Check "the new place of User0 (U+10EC80) runs bind -n User0" (Opt $ns '@hu') "yes"
            try { Stop-Process -Id $c.Id -Force -EA SilentlyContinue } catch {}
        } else {
            Write-Fail "[3] the pane shell never showed a prompt"
        }
        & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    }
}
catch {
    Write-Fail "exception: $_"
}
finally {
    foreach ($id in $script:OpenedPids) { try { Stop-Process -Id $id -Force -EA SilentlyContinue } catch {} }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
