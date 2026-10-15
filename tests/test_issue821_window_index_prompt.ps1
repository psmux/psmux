# Issue #821: prefix + ' opens tmux's window index prompt, a plain
# command-prompt (key-bindings.c:364, tag 3.7c):
#
#   bind -N 'Prompt for window index to select' "'" {
#       command-prompt -T window-target -pindex { select-window -t ':%%' }
#   }
#
# psmux opened a box of the client's own for it. The box took printable
# characters, Backspace, Enter and Escape and nothing else, sent every other key
# on to the pane, let the prefix arm while it was open, and was not redrawn as
# it was typed into. Measured in tmux 3.7c through a nested client, after
# `prefix '`:
#
#   12, Left, 3, |              index 13|2
#   45, C-a, x                  index x45
#   45, C-u, 2                  index 2
#   45, C-w, 1                  index 1
#   2, C-b, 1                   index 12    (C-b is cursor left at a prompt)
#   2, Enter                    window 2
#   Up, Enter, after answer 1   window 1
#   C-y (buffer 3), Enter       window 3
#   named, Enter                the window named "named"
#
# and none of those keys reach the pane.
#
# GROUND TRUTH: the text of the attached client's console (tests\conread.cs)
# for what the prompt holds, `capture-pane` for what reached the pane, and
# `display-message` for the window selected. Keys go in through
# tests\injector.cs.
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

$NS   = "wi-" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$SESS = "wi"
$TMP  = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_wi_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
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

$CONF = Join-Path $TMP "wi.conf"
$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
@(
    "set -g default-shell $POWERSHELL",
    "set -g status-left ''",
    # The old name, bound to a key of its own, for section 6.
    "bind-key F2 select-window-index"
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

# What the prompt HOLDS, read off the screen. The prompt is a box titled
# `index`, the label `-p index` gives it, with the text on the row below the
# title after ": ". The border is dropped whether it comes back as box drawing
# characters or, through an ANSI code page best fit, as `+` and `-`. Every case
# ends its text with a visible character, so trimming loses nothing.
function Prompt-Value {
    $screen = Screen
    for ($i = 0; $i -lt $screen.Count; $i++) {
        $title = (([string]$screen[$i]) -replace '[^ -~]', '' -replace '[+-]', '').Trim()
        if ($title -eq 'index' -and $i + 1 -lt $screen.Count) {
            $line = (([string]$screen[$i + 1]) -replace '[^ -~]', '').TrimEnd()
            $at = $line.IndexOf(':')
            if ($at -ge 0) { return $line.Substring($at + 1).Trim() }
            return ""
        }
    }
    return $null   # no prompt on screen
}

function Pane-Text {
    $cap = (P capture-pane -t $SESS -p) -join "`n"
    return ($cap -split "`n" | Where-Object { $_.Trim() }) -join " | "
}

function Window-Index { return ((P display-message -t $SESS -p '#{window_index}') -join '').Trim() }

function Select-First {
    P select-window -t "${SESS}:0" | Out-Null
    Start-Sleep -Milliseconds 300
}

function Open-Prompt { Inj "^b{SLEEP:300}'{SLEEP:400}" }

function Close-Prompt {
    Inj "{ESC}"
    Start-Sleep -Milliseconds 300
}

# Open the prompt, press `keys`, and read what it holds.
function Prompt-After($keys) {
    Open-Prompt
    Inj $keys
    $v = Prompt-Value
    Close-Prompt
    return $v
}

function Case($label, $want, $v) {
    Check ($v -ceq $want) "${label}: [$v]" "${label}: wanted [$want], the prompt reads [$v]"
}

# -- 1. the binding --

Write-Head "1. list-keys shows tmux's command"
Kill-Rig
$proc = Start-Attached
if (-not $proc) {
    Write-Fail "the attached client never came up"
    Kill-Rig
    Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
    exit 1
}
$script:cpid = $proc.Id
$line = (P list-keys -T prefix) | Where-Object { "$_" -match "^bind-key\s+-T prefix\s+\\?'\s" } | Select-Object -First 1
Write-Info "list-keys: $line"
Check ("$line" -match "command-prompt -T window-target -p index" -and "$line" -match "select-window -t ':%%'") `
    "prefix ' is command-prompt -T window-target -p index ... select-window -t ':%%'" `
    "prefix ' is [$line]"

# Three more windows and one with a name: 0 1 2 3 4(named).
P new-window -d -t "${SESS}:" | Out-Null
P new-window -d -t "${SESS}:" | Out-Null
P new-window -d -t "${SESS}:" | Out-Null
P new-window -d -t "${SESS}:" -n named | Out-Null
Start-Sleep -Milliseconds 800
P send-keys -t "${SESS}:0" "echo MARKER_ONE" Enter | Out-Null
Start-Sleep -Seconds 2
Select-First
$script:paneBefore = Pane-Text

# -- 2. what is typed is drawn --

Write-Head "2. the prompt shows what is typed, with nothing else bringing a frame"
Open-Prompt
Inj "1"
Inj "2"
$v = Prompt-Value
Close-Prompt
Case "1, 2" "12" $v

# -- 3. the prompt editor --

Write-Head "3. the editing keys of tmux's prompt"
Case "12, Left, 3, |" "13|2" (Prompt-After "12{LEFT}3|")
Case "45, C-a, x" "x45" (Prompt-After "45^ax")
Case "45, C-u, 2" "2" (Prompt-After "45^u2")
Case "45, C-w, 1" "1" (Prompt-After "45^w1")
Case "2, C-b, 1 (C-b is cursor left, not the prefix)" "12" (Prompt-After "2^b1")

# -- 4. nothing reached the pane --

Write-Head "4. the keys stay at the prompt"
Open-Prompt
Inj "^u"
Inj "{UP}"
Close-Prompt
$after = Pane-Text
Check ($after -eq $script:paneBefore) "the pane is unchanged after C-u, Up, Escape" "the pane changed after C-u, Up, Escape"

# -- 5. Enter selects --

Write-Head "5. Enter selects the window"
Select-First
Open-Prompt
Inj "2{ENTER}"
Start-Sleep -Milliseconds 300
$w = Window-Index
Check ($w -eq "2") "2, Enter selects window 2" "2, Enter left window [$w] current"

Select-First
Open-Prompt
Inj "named{ENTER}"
Start-Sleep -Milliseconds 300
$w = Window-Index
Check ($w -eq "4") "named, Enter selects the window named named (4)" "named, Enter left window [$w] current"

Select-First
Open-Prompt
Inj "1{ENTER}"
Start-Sleep -Milliseconds 300
Select-First
Open-Prompt
Inj "{UP}"
$v = Prompt-Value
Case "Up after the answer 1" "1" $v
Inj "{ENTER}"
Start-Sleep -Milliseconds 300
$w = Window-Index
Check ($w -eq "1") "Up, Enter selects window 1" "Up, Enter left window [$w] current"

# C-y inserts the top paste buffer, as at any tmux prompt (#815).
Select-First
P set-buffer 3 | Out-Null
Open-Prompt
Inj "^y"
$v = Prompt-Value
Case "C-y with the buffer 3" "3" $v
Inj "{ENTER}"
Start-Sleep -Milliseconds 300
$w = Window-Index
Check ($w -eq "3") "C-y, Enter selects window 3" "C-y, Enter left window [$w] current"

Select-First
$v = Prompt-After "9"
Case "Escape and reopen: the prompt opens empty, 9" "9" $v
$w = Window-Index
Check ($w -eq "0") "Escape selects nothing" "Escape left window [$w] current"

# -- 6. the old name --

Write-Head "6. a key bound to select-window-index opens the same prompt"
Select-First
Inj "^b{SLEEP:300}{F2}{SLEEP:400}"
Inj "3"
$v = Prompt-Value
Case "prefix F2, 3" "3" $v
Inj "{ENTER}"
Start-Sleep -Milliseconds 300
$w = Window-Index
Check ($w -eq "3") "prefix F2, 3, Enter selects window 3" "prefix F2, 3, Enter left window [$w] current"

# -- Done --

Kill-Rig
Remove-Item $TMP -Recurse -Force -EA SilentlyContinue

Write-Host ""
Write-Host "passed: $script:TestsPassed  failed: $script:TestsFailed" -ForegroundColor Cyan
exit $script:TestsFailed
