# Issue #783: the client's command prompt takes the keys tmux's prompt takes,
# and lets none of them reach the pane behind it.
#
# tmux gives every prompt the same editor (status.c `status_prompt_key`), and
# its `default` case appends the key to the buffer (status.c:1704), so no key
# pressed at a prompt reaches a pane. psmux has that editor too,
# `copy_mode::prompt_edit`, and the copy mode prompts and the rename overlay
# use it. The command prompt of an attached client had its own arms instead:
#
#   * `C-b`, `C-f`, `C-d`, `C-h`, `C-p`, `C-n`, `C-c` and `C-g` had no arm at
#     all, so they fell through to the client's pane key handling and were sent
#     to the focused pane as `send-key C-<letter>`. `C-l` cleared the shell's
#     screen behind the prompt and `C-p` recalled the shell's own history.
#   * `C-u` deleted from the cursor to the start of the line, where tmux
#     deletes the whole line (status.c:1548), which is what the manual's
#     "Delete entire command" says.
#
# Measured in discussion #782, key by key, against tmux 3.7c.
#
# GROUND TRUTH: the text of the attached client's console (tests\conread.cs)
# for what the prompt holds, and `capture-pane` for what reached the pane.
# Keys go in through tests\injector.cs.
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

$NS   = "pe-" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$SESS = "pe"
$TMP  = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_pe_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null

function P { & $PSMUX -L $NS @args 2>&1 }

$csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue |
           Select-Object -First 1 -ExpandProperty FullName
}
foreach ($tool in "conread", "injector", "paste_host_injector") {
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

$CONF = Join-Path $TMP "pe.conf"
$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
@(
    "set -g default-shell $POWERSHELL",
    "set -g history-limit 2000",
    "set -g status-left ''"
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

# -- 1. the cursor keys the prompt did not have --

Write-Head "1. C-b and C-f move the cursor"
Open-Prompt "ac"
Inj "^b"
$v = Prompt-Value
Check ($v -eq 'ac') "C-b left the text alone [ac]" "after C-b the prompt reads [$v]"
Inj "b"
$v = Prompt-Value
Check ($v -eq 'abc') "typing after C-b lands before the c [abc]" "the prompt reads [$v]"
Inj "^f"
Inj "d"
$v = Prompt-Value
Check ($v -eq 'abcd') "C-f moves back towards the end [abcd]" "the prompt reads [$v]"

Write-Head "2. C-h and C-d delete either side of the cursor"
Inj "^b"
Inj "^h"
$v = Prompt-Value
Check ($v -eq 'abd') "C-h takes the character before the cursor [abd]" "the prompt reads [$v]"
Inj "^d"
$v = Prompt-Value
Check ($v -eq 'ab') "C-d takes the character at the cursor [ab]" "the prompt reads [$v]"

Write-Head "3. C-a and C-e jump to either end"
Inj "^a"
Inj "x"
$v = Prompt-Value
Check ($v -eq 'xab') "C-a parks the cursor before the first character [xab]" "the prompt reads [$v]"
Inj "^e"
Inj "y"
$v = Prompt-Value
Check ($v -eq 'xaby') "C-e parks it after the last one [xaby]" "the prompt reads [$v]"

# -- 4. C-u is the whole line, as the manual says --

Write-Head "4. C-u deletes the entire command"
Inj "^b{SLEEP:200}^b"
Inj "^u"
$v = Prompt-Value
Check ($v -eq '') "C-u with the cursor inside the line left it empty" "the prompt still reads [$v]"

# -- 5. nothing typed at the prompt reaches the pane --

Write-Head "5. a key the prompt has no use for stops at the prompt"
Inj "ls"
$v = Prompt-Value
Inj "^l"
Start-Sleep -Milliseconds 600
$after = Pane-Text
Check ($after -eq $script:paneBefore) "C-l did not reach the pane" "the pane changed: [$after]"
Check ((Prompt-Value) -eq $v) "C-l left the prompt's own text alone [$v]" "the prompt reads [$(Prompt-Value)]"

Inj "^p"
Start-Sleep -Milliseconds 600
$after = Pane-Text
Check ($after -eq $script:paneBefore) "C-p did not reach the pane either" "the pane changed: [$after]"

Inj "{F5}"
Start-Sleep -Milliseconds 400
$after = Pane-Text
Check ($after -eq $script:paneBefore) "F5 did not reach the pane" "the pane changed: [$after]"
Check ((Prompt-Value) -eq $v) "and F5 left the prompt's text alone [$v]" "the prompt reads [$(Prompt-Value)]"

Inj "{PGUP}"
Start-Sleep -Milliseconds 400
$after = Pane-Text
Check ($after -eq $script:paneBefore) "PageUp did not reach the pane" "the pane changed: [$after]"

# -- 6. C-c and C-g cancel the prompt, as Escape does --

Write-Head "6. C-c cancels the prompt"
Inj "^c"
Start-Sleep -Milliseconds 500
Check (-not (Prompt-Open)) "the prompt closed on C-c" "the prompt is still open, reading [$(Prompt-Value)]"
$after = Pane-Text
Check ($after -eq $script:paneBefore) "and C-c did not reach the pane as an interrupt" "the pane changed: [$after]"

Write-Head "7. C-g cancels it too"
Open-Prompt "half typed"
Check (Prompt-Open) "the prompt is open again" "the prompt did not open"
Inj "^g"
Start-Sleep -Milliseconds 500
Check (-not (Prompt-Open)) "the prompt closed on C-g" "the prompt is still open, reading [$(Prompt-Value)]"

# -- 8. what already worked still works --

Write-Head "8. the named keys still edit the line"
Open-Prompt "ac"
Inj "{LEFT}"
Inj "b"
$v = Prompt-Value
Check ($v -eq 'abc') "Left then typing lands before the c [abc]" "the prompt reads [$v]"
Inj "{HOME}"
Inj "z"
$v = Prompt-Value
Check ($v -eq 'zabc') "Home parks the cursor at the start [zabc]" "the prompt reads [$v]"
Inj "{END}"
Inj "q"
$v = Prompt-Value
Check ($v -eq 'zabcq') "End parks it after the last character [zabcq]" "the prompt reads [$v]"
Close-Prompt
Check (-not (Prompt-Open)) "Escape still cancels the prompt" "the prompt is still open"

Write-Head "9. the prompt still runs the command it was edited into"
P rename-window -t $SESS "before" | Out-Null
Start-Sleep -Milliseconds 400
# `rename-window AC`, then C-b and a `B`, which makes it `rename-window ABC`.
Open-Prompt "rename-window AC"
Inj "^b"
Inj "B"
$v = Prompt-Value
Write-Info "the prompt holds: [$v]"
Inj "{ENTER}"
Start-Sleep -Seconds 1
$name = ((P display-message -t $SESS -p '#{window_name}') -join '').Trim()
Check ($name -eq 'ABC') "the edited command ran [ABC]" "the window is called [$name]"

Write-Head "10. the history is one C-p away"
Open-Prompt ""
Inj "^p"
$v = Prompt-Value
Check ($v -eq 'rename-window ABC') "C-p recalled the last command" "the prompt reads [$v]"
Inj "^n"
$v = Prompt-Value
Check ($v -eq '') "C-n steps back to an empty line" "the prompt reads [$v]"
Close-Prompt

# -- 11. the host's duplicate of a paste stays out of the line --
#
# A host that pastes on its own AND forwards the characters (#290) delivers
# the text twice: once as the clipboard paste the prompt takes whole, then as
# key events inside the client's paste suppress window, which are dropped.
# The catch all arm used to hand those to the prompt editor, which types a
# Space as a space, so "a b c d e f" left five extra spaces in the line. The
# letters were dropped, so a run of two spaces is the tell: the paste itself
# has none, and whatever of the duplicate arrives after the window closes
# starts with a letter or a single space.

Write-Head "11. a paste's duplicate key events add nothing to the line"
$savedClip = try { Get-Clipboard -Raw -EA Stop } catch { $null }
$clipOk = $false
try { Set-Clipboard -Value "a b c d e f" -EA Stop; $clipOk = ((Get-Clipboard -Raw) -eq "a b c d e f") } catch {}
if (-not $clipOk) {
    Write-Info "SKIP: the desktop refuses clipboard access to this process"
} else {
    Open-Prompt "X"
    $PASTE = Join-Path $TMP "paste_host_injector.exe"
    & $PASTE $script:cpid plain 20 "a b c d e f" | Out-Null
    & $INJ $script:cpid "a b c d e f" | Out-Null
    Start-Sleep -Milliseconds 1200
    # A closing letter, because the reader trims the end of the line and the
    # spaces would be typed at the end.
    Inj "Y"
    $v = Prompt-Value
    Write-Info "the prompt holds: [$v]"
    Check ($v -like 'Xa b c d e f*Y') "the paste arrived in the prompt [$v]" "the prompt reads [$v]"
    Check ($v -notmatch '  ') "no duplicate space was typed into the line" "the line gained duplicate spaces: [$v]"
    Close-Prompt
    if ($null -ne $savedClip) { Set-Clipboard -Value $savedClip }
}

# -- Done --

Kill-Rig
Remove-Item $TMP -Recurse -Force -EA SilentlyContinue

Write-Host ""
Write-Host "passed: $script:TestsPassed  failed: $script:TestsFailed" -ForegroundColor Cyan
if ($script:TestsFailed -gt 0) { exit 1 }
exit 0
