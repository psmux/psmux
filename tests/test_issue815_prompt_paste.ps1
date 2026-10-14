# Issue #815: C-y at the prompts inserts what C-w last cut, or else the top
# paste buffer, as it does at tmux's prompt (status_prompt_paste, status.c:1186
# to :1245, called from status.c:1645, tag 3.7c). Discussion #782, item 1.
#
# tmux inserts the cut if C-w made one since the prompt opened, and otherwise
# the newest automatic buffer up to its first control character, so only the
# first line. The cursor ends after the inserted text. Measured in tmux 3.7c
# through a nested client, with status-keys emacs:
#
#   abc, C-y (buffer BUF1)                       :abcBUF1
#   abc, C-a, C-y, |                             :BUF1|abc
#   ac, Left, C-y (buffer B), X                  :aBXc
#   buffer line1<LF>line2, C-y, |                :line1|
#   x, C-y (buffer '  sp  '), |                  :x  sp  |
#   set-buffer AUTO, -b named NAMED, C-y, |      :AUTO|
#   no buffer, q, C-y, |                         :q|
#   one two, C-w, C-y, C-y                       :one twotwo
#   empty, C-w, C-y, | (buffer BUF1)             :|
#   aa bb, C-w, Escape, reopen, C-y              :BUF1
#   copy mode search /, s, C-y (buffer SRCH)     (search down) sSRCH
#
# psmux's prompts share one editor, `copy_mode::prompt_edit`, so the command
# prompt, the rename overlay and the copy mode search prompt are all checked.
# The first two are drawn by the client, which asks the server for the buffer.
#
# GROUND TRUTH: the text of the attached client's console (tests\conread.cs)
# for what a prompt holds, `capture-pane` for what reached the pane, and
# `display-message` for what a command edited at the prompt did. Keys go in
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

$NS   = "cy-" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$SESS = "cy"
$TMP  = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_cy_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
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

$CONF = Join-Path $TMP "cy.conf"
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
# read. The prompt's own row is the one below the box title. Every case ends
# its text with a visible character, so trimming the row loses no space that
# belongs to the prompt.
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

# Remove every buffer, so each case starts from what it sets. A bare
# `delete-buffer` removes the top automatic buffer; the one named buffer this
# suite makes goes by its name.
function Clear-Buffers {
    P delete-buffer -b named | Out-Null
    for ($i = 0; $i -lt 20; $i++) {
        $left = @(P list-buffers | Where-Object { "$_".Trim() })
        if ($left.Count -eq 0) { return }
        P delete-buffer | Out-Null
    }
}

function Set-Buffer($text) {
    Clear-Buffers
    P set-buffer $text | Out-Null
}

# A buffer with more than one line goes in through a file, so the line break
# is part of the buffer and not of the command line.
function Load-Buffer($text) {
    Clear-Buffers
    $f = Join-Path $TMP "buf.txt"
    [System.IO.File]::WriteAllText($f, $text)
    P load-buffer $f | Out-Null
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

function Case($label, $want, $v) {
    Check ($v -ceq $want) "${label}: [$v]" "${label}: wanted [$want], the prompt reads [$v]"
}

# -- 1. the top paste buffer at the command prompt --

Write-Head "1. C-y inserts the top paste buffer at the command prompt"
Set-Buffer "BUF1"
Case "abc, C-y (buffer BUF1)" "abcBUF1" (Prompt-After "abc" "^y")
Case "abc, Home, C-y, |" "BUF1|abc" (Prompt-After "abc" "{HOME}^y|")
Set-Buffer "B"
Case "ac, Left, C-y (buffer B), X" "aBXc" (Prompt-After "ac" "{LEFT}^yX")
Load-Buffer "line1`nline2"
Case "buffer line1<LF>line2, C-y, |" "line1|" (Prompt-After "" "^y|")
Set-Buffer "  sp  "
Case "x, C-y (buffer '  sp  '), |" "x  sp  |" (Prompt-After "x" "^y|")
Set-Buffer "AUTO"
P set-buffer -b named NAMED | Out-Null
Case "set-buffer AUTO then -b named NAMED, C-y, |" "AUTO|" (Prompt-After "" "^y|")
Clear-Buffers
Case "no buffer, q, C-y, |" "q|" (Prompt-After "q" "^y|")

# -- 2. what C-w cut --

Write-Head "2. C-y inserts what C-w cut, for as long as the prompt is open"
Set-Buffer "BUF1"
Case "one two, C-w, C-y, C-y" "one twotwo" (Prompt-After "one two" "^w{SLEEP:150}^y{SLEEP:150}^y")
Case "empty, C-w, C-y, |" "|" (Prompt-After "" "^w{SLEEP:150}^y|")
Open-Prompt "aa bb"
Inj "^w"
Close-Prompt
Case "aa bb, C-w, Escape, reopen, C-y" "BUF1" (Prompt-After "" "^y")

# -- 3. nothing reached the pane --

Write-Head "3. C-y stays at the prompt"
$after = Pane-Text
Check ($after -eq $script:paneBefore) "the pane is unchanged" "the pane changed: [$after]"

# -- 4. the command it was pasted into runs --

Write-Head "4. a command completed with C-y runs"
Set-Buffer "PASTED"
Open-Prompt "rename-window "
Inj "^y"
$v = Prompt-Value
Check ($v -ceq "rename-window PASTED") "the prompt reads [$v]" "the prompt reads [$v]"
Inj "{ENTER}"
Start-Sleep -Milliseconds 500
$w = Window-Name
Check ($w -ceq "PASTED") "the window is PASTED" "the window reads [$w]"

# -- 5. the rename overlay --

Write-Head "5. C-y in the rename overlay"
Set-Buffer "RENAMED"
Inj "^b{SLEEP:300},{SLEEP:500}"
Inj "^u"
Inj "^y"
$v = Rename-Value
Check ($v -ceq "RENAMED") "the name reads [$v]" "the name reads [$v]"
Inj "{ENTER}"
Start-Sleep -Milliseconds 500
$w = Window-Name
Check ($w -ceq "RENAMED") "the window is RENAMED" "the window reads [$w]"

# -- 6. the copy mode search prompt --

Write-Head "6. C-y at the copy mode search prompt"
Set-Buffer "SRCH"
P copy-mode -t $SESS | Out-Null
Start-Sleep -Milliseconds 500
Inj "/"
Inj "s"
Inj "^y"
$v = Search-Value
Check ($v -ceq "sSRCH") "the search prompt reads [$v]" "the search prompt reads [$v]"
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
