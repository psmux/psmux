# Issue #785: C-t at the prompts transposes the two characters at the cursor,
# as it does at tmux's prompt (status.c:1649 to :1661, tag 3.7c). Discussion
# #782, item 4.
#
# tmux steps the cursor right first unless it is at the end, then swaps the
# two characters before it: inside the line the characters either side of the
# cursor change places, at the end the last two do, and the cursor ends after
# the pair. At the start nothing changes. Measured in tmux 3.7c through a
# nested client, with status-keys emacs:
#
#   abc, at the end, C-t           :acb
#   abc, at the end, C-t C-t       :abc
#   abcd, Left Left, C-t, X        :acbXd
#   abc, Home, C-t, X              :Xabc
#   ab, Home Right, C-t, X         :baX
#   a, C-t, X                      :aX
#   search prompt, abc, C-t        (search up) acb
#
# psmux's prompts share one editor, `copy_mode::prompt_edit`, so the command
# prompt, the rename overlay and the copy mode search prompt are all checked.
#
# GROUND TRUTH: the text of the attached client's console (tests\conread.cs)
# for what a prompt holds, `capture-pane` for what reached the pane, and
# `list-windows` for what a command edited at the prompt did. Keys go in
# through tests\injector.cs.
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

$NS   = "ct-" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$SESS = "ct"
$TMP  = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_ct_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
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

$CONF = Join-Path $TMP "ct.conf"
$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
@(
    "set -g default-shell $POWERSHELL",
    "set -g history-limit 2000",
    "set -g status-left ''",
    "set -g mode-keys vi"
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

# What the prompt HOLDS. The box is drawn with border characters, which the
# screen reader returns as non ASCII, so they are dropped before the line is
# read. The prompt's own row is the one below the box title.
function Prompt-Value {
    $screen = Screen
    for ($i = 0; $i -lt $screen.Count; $i++) {
        # The border is dropped whether it comes back as box drawing characters
        # or, through an ANSI code page best fit, as `+` and `-`.
        $title = (([string]$screen[$i]) -replace '[^ -~]', '' -replace '[+-]', '').Trim()
        if ($title -eq 'command' -and $i + 1 -lt $screen.Count) {
            $line = (([string]$screen[$i + 1]) -replace '[^ -~]', '').TrimEnd()
            $at = $line.IndexOf(':')
            if ($at -ge 0) { return $line.Substring($at + 1).Trim() }
            return ""
        }
    }
    return $null   # no prompt on screen
}

function Prompt-Open { return ($null -ne (Prompt-Value)) }

function Pane-Text {
    $cap = (P capture-pane -t $SESS -p) -join "`n"
    return ($cap -split "`n" | Where-Object { $_.Trim() }) -join " | "
}

function Open-Prompt($text) {
    Inj "^b{SLEEP:300}:{SLEEP:400}"
    if ($text) { Inj $text }
}

function Close-Prompt {
    Inj "{ESC}"
    Start-Sleep -Milliseconds 300
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


# What the rename overlay holds: the row that reads "name: ...". The border
# comes back from the screen reader as non ASCII, so it is dropped first.
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

function Window-Name { return ((P display-message -t $SESS -p '#{window_name}') -join '').Trim() }

# Open the command prompt on `text`, press `keys`, and read what it holds.
function Prompt-After($text, $keys) {
    Open-Prompt $text
    Inj $keys
    $v = Prompt-Value
    Close-Prompt
    return $v
}

# -- 1. the command prompt --

Write-Head "1. C-t at the command prompt"
$cases = @(
    @{ Label = "abc, at the end, C-t";       Text = "abc";  Keys = "^t";                  Want = "acb"   },
    @{ Label = "abc, at the end, C-t C-t";   Text = "abc";  Keys = "^t{SLEEP:150}^t";     Want = "abc"   },
    @{ Label = "abcd, Left Left, C-t, X";    Text = "abcd"; Keys = "{LEFT}{LEFT}^tX";     Want = "acbXd" },
    @{ Label = "abc, Home, C-t, X";          Text = "abc";  Keys = "{HOME}^tX";           Want = "Xabc"  },
    @{ Label = "ab, Home Right, C-t, X";     Text = "ab";   Keys = "{HOME}{RIGHT}^tX";    Want = "baX"   },
    @{ Label = "a, C-t, X";                  Text = "a";    Keys = "^tX";                 Want = "aX"    }
)
foreach ($c in $cases) {
    $v = Prompt-After $c.Text $c.Keys
    Check ($v -ceq $c.Want) "$($c.Label): [$v]" "$($c.Label): wanted [$($c.Want)], the prompt reads [$v]"
}

# -- 2. nothing reached the pane --

Write-Head "2. C-t stays at the prompt"
$after = Pane-Text
Check ($after -eq $script:paneBefore) "the pane is unchanged" "the pane changed: [$after]"

# -- 3. the command it was edited into runs --

Write-Head "3. a command fixed with C-t runs as fixed"
Open-Prompt "rename-window TSET"
Inj "{LEFT}{LEFT}^t"
$v = Prompt-Value
Check ($v -ceq "rename-window TEST") "the prompt reads [$v]" "the prompt reads [$v]"
Inj "{ENTER}"
Start-Sleep -Milliseconds 500
$w = Window-Name
Check ($w -ceq "TEST") "the window is TEST" "the window reads [$w]"

# -- 4. the rename overlay --

Write-Head "4. C-t in the rename overlay"
Inj "^b{SLEEP:300},{SLEEP:500}"
Inj "^u"
Inj "ALHPA{LEFT}{LEFT}"
Inj "^t"
$v = Rename-Value
Check ($v -ceq "ALPHA") "the name reads [$v]" "the name reads [$v]"
Inj "{ENTER}"
Start-Sleep -Milliseconds 500
$w = Window-Name
Check ($w -ceq "ALPHA") "the window is ALPHA" "the window reads [$w]"

# -- 5. the copy mode search prompt --

Write-Head "5. C-t at the copy mode search prompt"
P copy-mode -t $SESS | Out-Null
Start-Sleep -Milliseconds 500
Inj "/"
Inj "abc"
Inj "^t"
$v = Search-Value
Check ($v -ceq "acb") "the search prompt reads [$v]" "the search prompt reads [$v]"
Inj "{ESC}"
Inj "q"
Start-Sleep -Milliseconds 300
$inMode = ((P display-message -t $SESS -p '#{pane_in_mode}') -join '').Trim()
if ($inMode -ne "0") { P send-keys -t $SESS -X cancel | Out-Null }

# -- Done --

Kill-Rig
Remove-Item $TMP -Recurse -Force -EA SilentlyContinue

Write-Host ""
Write-Host "passed: $script:TestsPassed  failed: $script:TestsFailed" -ForegroundColor Cyan
if ($script:TestsFailed -gt 0) { exit 1 }
exit 0
