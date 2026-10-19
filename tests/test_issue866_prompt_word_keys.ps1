# Issue #866: the prompts move by word on M-f, C-Right, M-b and C-Left, and
# C-w cuts a word split the same way, with the word-separators option, as
# tmux's prompt does (status_prompt_forward_word and
# status_prompt_backward_word, status.c:1320 and :1399, the keys at :1558,
# :1598 and :1620, tag 3.7c). Discussion #782, items 3 and 8.
#
# Measured in tmux 3.7c through a nested client, with the default
# word-separators, typing X after the keys so the cursor shows:
#
#   ls /usr/local, Home, M-f M-f, X              :ls /Xusr/local
#   ls /usr/local, M-b M-b, X                    :ls /usrX/local
#   ls /usr/local, Home, C-Right x3, X           :ls /usrX/local
#   ls /usr/local, C-Left, X                     :ls /usr/Xlocal
#   ls /usr/local, C-w                           :ls /usr/
#   ls /usr/local, C-w C-w                       :ls /usr
#   foo-bar_baz qux, Home, M-f x3, X             :foo-bar_bazX qux
#
# tmux drops an Alt key it has no use for rather than typing it
# (status.c:1710 to :1718). The suite sets word-separators to "-./", which
# splits these lines the way tmux's default does, so it does not depend on
# psmux's default.
#
# psmux's prompts share one editor, `copy_mode::prompt_edit`, so the command
# prompt, the rename overlay and the copy mode search prompt are all checked.
#
# GROUND TRUTH: the text of the attached client's console (tests\conread.cs)
# for what a prompt holds, `capture-pane` for what reached the pane, and
# `display-message` for what a command edited at the prompt did. Keys go in
# through tests\injector.cs, Ctrl+Left and Ctrl+Right as raw records with the
# enhanced key flag a real keyboard sets on the arrows.
#
# Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$script:TestsPassed = 0; $script:TestsFailed = 0
$script:Opened = @()

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

$env:PSMUX_SESSION_NAME = $null
$env:PSMUX_SESSION      = $null
$env:PSMUX_PANE         = $null
$env:TMUX               = $null
$env:TMUX_PANE          = $null

$NS   = "pw-" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$SESS = "pw"
$TMP  = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_pw_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null

function P { & $PSMUX -L $NS @args 2>&1 }

$csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue |
           Select-Object -First 1 -ExpandProperty FullName
}
foreach ($tool in "conread", "injector") {
    $exe = Join-Path $TMP "$tool.exe"
    if ($csc -and (Test-Path $csc)) {
        & $csc /nologo /optimize /out:$exe (Join-Path $PSScriptRoot "$tool.cs") 2>&1 | Out-Null
    }
    if (-not (Test-Path $exe)) {
        Write-Fail "could not build tests\$tool.cs (csc.exe unavailable)"
        Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
        exit 1
    }
}
$RD  = Join-Path $TMP "conread.exe"
$INJ = Join-Path $TMP "injector.exe"

function Stop-Opened {
    foreach ($id in $script:Opened) { try { Stop-Process -Id $id -Force -EA SilentlyContinue } catch {} }
    $script:Opened = @()
}

function Kill-Rig {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    Stop-Opened
    Get-ChildItem "$psmuxDir\${NS}__*" -EA SilentlyContinue | Remove-Item -Force -EA SilentlyContinue
}

$CONF = Join-Path $TMP "pw.conf"
$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
@(
    "set -g default-shell $POWERSHELL",
    "set -g history-limit 2000",
    "set -g status-left ''",
    "set -g mode-keys vi",
    "set -g word-separators '-./'"
) | Set-Content -Path $CONF -Encoding ASCII

function Start-Attached {
    $p = Start-Process -FilePath $PSMUX `
        -ArgumentList "-f",$CONF,"-L",$NS,"new-session","-s",$SESS,"-x","100","-y","30" -PassThru
    $script:Opened += $p.Id
    $portFile = Join-Path $psmuxDir "${NS}__${SESS}.port"
    for ($i = 0; $i -lt 80; $i++) {
        Start-Sleep -Milliseconds 250
        if (Test-Path $portFile) {
            $port = (Get-Content $portFile -Raw).Trim()
            try {
                $t = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port); $t.Close()
                Start-Sleep -Milliseconds 2500
                return $p
            } catch {}
        }
    }
    return $null
}

function Inj($keys) { & $INJ $script:cpid $keys | Out-Null; Start-Sleep -Milliseconds 450 }

function Screen {
    $o = Join-Path $TMP "screen.txt"
    Start-Process -FilePath $RD -ArgumentList "$script:cpid" -Wait -WindowStyle Hidden -RedirectStandardOutput $o | Out-Null
    if (Test-Path $o) { return @(Get-Content $o) }
    return @()
}

# What the prompt HOLDS: the row below the box title. The border comes back
# from the screen reader as non ASCII, or as `+` and `-` through a code page
# best fit, so it is dropped before the title is read.
function Prompt-Value {
    $screen = Screen
    for ($i = 0; $i -lt $screen.Count; $i++) {
        $title = (([string]$screen[$i]) -replace '[^ -~]', '' -replace '[+-]', '').Trim()
        if ($title -eq 'command' -and $i + 1 -lt $screen.Count) {
            $line = (([string]$screen[$i + 1]) -replace '[^ -~]', '').TrimEnd()
            $at = $line.IndexOf(':')
            if ($at -ge 0) { return $line.Substring($at + 1).Trim() }
            return ""
        }
    }
    return $null
}

# What the rename overlay holds: the row that reads "name: ...".
function Rename-Value {
    foreach ($l in (Screen)) {
        $line = [string]$l
        $at = $line.IndexOf("name: ")
        if ($at -ge 0) { return ($line.Substring($at + 6) -replace '[^ -~]', ' ').Trim() }
    }
    return $null
}

# The search prompt is drawn on the status line as "(search down) <text>".
function Search-Value {
    foreach ($l in (Screen)) {
        $line = ([string]$l) -replace '[^ -~]', ' '
        if ($line -match '\(search down\) ?(.*)$') { return $Matches[1].Trim() }
    }
    return $null
}

function Pane-Text {
    $cap = (P capture-pane -t $SESS -p) -join "`n"
    return ($cap -split "`n" | Where-Object { $_.Trim() }) -join " | "
}

function Window-Name { return ((P display-message -t $SESS -p '#{window_name}') -join '').Trim() }

$CLEFT  = "{RAW:25:00:0108}"
$CRIGHT = "{RAW:27:00:0108}"

# Open the command prompt on `text`, press `keys`, and read what it holds.
function Prompt-After($text, $keys) {
    Inj "^b{SLEEP:300}:{SLEEP:400}"
    Inj $text
    Inj $keys
    $v = Prompt-Value
    Inj "{ESC}"
    Start-Sleep -Milliseconds 300
    return $v
}

# -- Rig --

Kill-Rig
$proc = Start-Attached
if (-not $proc) {
    Write-Fail "the attached client never came up"
    Kill-Rig
    Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
    exit 1
}
$script:cpid = $proc.Id
Start-Sleep -Milliseconds 800
P send-keys -t $SESS "echo MARKER_ONE" Enter | Out-Null
Start-Sleep -Seconds 2
$script:paneBefore = Pane-Text
Write-Info "the pane holds: $script:paneBefore"
$seps = ((P show -gv word-separators) -join '').Trim()
Check ($seps -eq '-./') "word-separators is [-./]" "word-separators is [$seps]"

# -- 1. the command prompt --

Write-Head "1. the command prompt, against tmux 3.7c"
$cases = @(
    @{ Label = "Home, M-f M-f";      Text = "ls /usr/local";   Keys = "{HOME}{ALT:f}{ALT:f}X";              Want = "ls /Xusr/local" },
    @{ Label = "M-b M-b";            Text = "ls /usr/local";   Keys = "{ALT:b}{ALT:b}X";                    Want = "ls /usrX/local" },
    @{ Label = "Home, C-Right x3";   Text = "ls /usr/local";   Keys = "{HOME}$CRIGHT$CRIGHT${CRIGHT}X";     Want = "ls /usrX/local" },
    @{ Label = "C-Left";             Text = "ls /usr/local";   Keys = "${CLEFT}X";                          Want = "ls /usr/Xlocal" },
    @{ Label = "C-w";                Text = "ls /usr/local";   Keys = "^w";                                 Want = "ls /usr/" },
    @{ Label = "C-w C-w";            Text = "ls /usr/local";   Keys = "^w{SLEEP:150}^w";                    Want = "ls /usr" },
    @{ Label = "Home, M-f x3";       Text = "foo-bar_baz qux"; Keys = "{HOME}{ALT:f}{ALT:f}{ALT:f}X";       Want = "foo-bar_bazX qux" },
    @{ Label = "Alt+x is not typed"; Text = "ab";              Keys = "{ALT:x}X";                           Want = "abX" }
)
foreach ($c in $cases) {
    $v = Prompt-After $c.Text $c.Keys
    Check ($v -ceq $c.Want) "$($c.Text), $($c.Label): [$v]" "$($c.Text), $($c.Label): wanted [$($c.Want)], the prompt reads [$v]"
}

Write-Head "2. the keys stay at the prompt"
$after = Pane-Text
Check ($after -eq $script:paneBefore) "the pane is unchanged" "the pane changed: [$after]"

Write-Head "3. a command edited by word runs as edited"
Inj "^b{SLEEP:300}:{SLEEP:400}"
Inj "rename-window ONE-TWO"
# The first C-w cuts TWO and the second the separator before it; split at
# spaces only, the first one would take the whole of ONE-TWO.
Inj "^w{SLEEP:150}^w"
$v = Prompt-Value
Check ($v -ceq "rename-window ONE") "C-w C-w: the prompt reads [$v]" "C-w C-w: the prompt reads [$v]"
Inj "{ENTER}"
Start-Sleep -Milliseconds 500
$w = Window-Name
Check ($w -ceq "ONE") "the window is ONE" "the window reads [$w]"

# -- 4. the rename overlay --

Write-Head "4. the rename overlay"
Inj "^b{SLEEP:300},{SLEEP:500}"
Inj "^u"
Inj "ALPHA-BETA"
Inj "{ALT:b}X"
$v = Rename-Value
Check ($v -ceq "ALPHA-XBETA") "M-b, X: [$v]" "M-b, X: the name reads [$v]"
Inj "{END}${CLEFT}${CLEFT}Y"
$v = Rename-Value
Check ($v -ceq "ALPHAY-XBETA") "End, C-Left C-Left, Y: [$v]" "End, C-Left C-Left, Y: the name reads [$v]"
Inj "{END}^w"
$v = Rename-Value
Check ($v -ceq "ALPHAY-") "End, C-w: [$v]" "End, C-w: the name reads [$v]"
Inj "{ESC}"
Start-Sleep -Milliseconds 300

# -- 5. the copy mode search prompt --

Write-Head "5. the copy mode search prompt"
P copy-mode -t $SESS | Out-Null
Start-Sleep -Milliseconds 500
Inj "/"
Inj "ls /usr/local"
Inj "{ALT:b}X"
$v = Search-Value
Check ($v -ceq "ls /usr/Xlocal") "M-b, X: [$v]" "M-b, X: the search prompt reads [$v]"
Inj "${CLEFT}${CLEFT}^w"
$v = Search-Value
Check ($v -ceq "ls //Xlocal") "C-Left C-Left, C-w: [$v]" "C-Left C-Left, C-w: the search prompt reads [$v]"
Inj "{ESC}"
Inj "q"
Start-Sleep -Milliseconds 300
$inMode = ((P display-message -t $SESS -p '#{pane_in_mode}') -join '').Trim()
if ($inMode -ne "0") { P send-keys -t $SESS -X cancel | Out-Null }

# -- Done --

Kill-Rig
Remove-Item $TMP -Recurse -Force -EA SilentlyContinue

Write-Host ""
Write-Host "=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
