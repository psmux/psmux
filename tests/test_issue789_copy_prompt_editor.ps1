# A prompt opened by a copy mode key table binding edits through the shared
# prompt editor, as the command prompt, the rename overlays and the copy mode
# search and goto line prompts already do.
#
# `copy_prompt::feed` had a line editor of its own: it appended characters,
# BSpace removed the last one, C-u cleared the line and every other key was
# dropped. The cursor never moved, C-a, C-e, C-k, C-w, C-t, Delete and the
# arrows did nothing, and Up and Down recalled no history. tmux sends this
# prompt's keys through the same status_prompt_key switch as any other
# prompt's (process_key, status.c:1483, tag 3.7c). Measured in tmux 3.7c
# through a nested client, with status-keys emacs and the bindings below:
#
#   Z, abc                          (edit) abc
#   Z, abc, C-u, x                  (edit) x
#   Z, abc, Left Left, X            (edit) aXbc
#   Z, bc, C-a, a, C-e, d           (edit) abcd
#   Z, ac, Home Right, b, End, d    (edit) abcd
#   Z, abXc, Left, BSpace           (edit) abc
#   Z, abXc, Left Left, Delete      (edit) abc
#   Z, abc def, Left Left, C-k      (edit) abc d
#   Z, abc def, C-w, X              (edit) abc X
#   Z, abc, Left, C-t               (edit) acb
#   Z, abc, Up                      (edit) abc, with nothing in the history
#   Z, abcd, Left Left              the cursor on column 2 of the text
#   Y, TST, Left Left, E, Enter     (name) TEST, and the window is renamed TEST
#
# With S bound as a -T search prompt, which shares the history of tmux's own
# `/` (also -T search, key-bindings.c:597):
#
#   /, line, Enter, then S, Up      (hist) line
#   S, MARK, Enter, then /, Up      (search down) MARK
#   S, Up Up, Down, Down            (hist) line, (hist) MARK, (hist)
#
# Each prompt was closed with C-c, and the pane stayed in copy mode.
#
# GROUND TRUTH: the text and the cursor of the attached client's console
# (tests\conread.cs) for what the prompt holds and where its cursor is,
# `display-message` for the window name and whether the pane is still in copy
# mode, and `capture-pane` for what reached the pane. Keys go in through
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

$NS   = "cpe-" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$SESS = "cpe"
$TMP  = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_cpe_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
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

# Z answers into a search, Y into rename-window, so Enter shows what the
# whole line held. The rename template is left unquoted: the in process
# command runner splits on whitespace and would keep quotes in the name,
# which is a separate matter from the prompt.
$CONF = Join-Path $TMP "cpe.conf"
$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
@(
    "set -g default-shell '$POWERSHELL -NoLogo -NoProfile'",
    "set -g history-limit 2000",
    "set -g status-left ''",
    "set -g mode-keys vi",
    "bind -T copy-mode-vi Z command-prompt -p '(edit)' `"send -X search-forward -- '%%'`"",
    "bind -T copy-mode-vi Y command-prompt -p '(name)' `"rename-window %%`"",
    "bind -T copy-mode-vi S command-prompt -T search -p '(hist)' `"send -X search-forward -- '%%'`""
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

# BSpace and Delete have no token of their own in the injector; MOD sends the
# key with no modifier records around it.
$BSPACE = "{MOD:08:0008:0000}"
$DELETE = "{MOD:2E:0000:0000}"

function Screen {
    $o = Join-Path $TMP "screen.txt"
    Start-Process -FilePath $RD -ArgumentList "$script:cpid","-c" -Wait -WindowStyle Hidden -RedirectStandardOutput $o | Out-Null
    if (Test-Path $o) { return @(Get-Content $o) }
    return @()
}

# The prompt is drawn on the status line as "<label> <text>". Returns the
# text, and the cursor's column counted from the start of the text, or $null
# when the prompt is not on screen.
function Prompt-State($label) {
    $screen = Screen
    $cx = -1; $cy = -1
    if ($screen.Count -gt 0 -and ([string]$screen[0]) -match '^CURSOR x=(\d+) y=(\d+)') {
        $cx = [int]$Matches[1]; $cy = [int]$Matches[2]
    }
    for ($i = 1; $i -lt $screen.Count; $i++) {
        $line = (([string]$screen[$i]) -replace '[^ -~]', ' ').TrimEnd()
        # The screen reader drops trailing blanks, so an empty prompt reads as
        # the label alone, with no space after it.
        $at = $line.IndexOf("$label ")
        if ($at -lt 0 -and $line.EndsWith($label)) { $at = $line.Length - $label.Length }
        if ($at -ge 0) {
            $start = $at + $label.Length + 1
            $col = if ($cy -eq $i - 1) { $cx - $start } else { $null }
            $text = if ($start -lt $line.Length) { $line.Substring($start).Trim() } else { "" }
            return @{ Text = $text; Cursor = $col }
        }
    }
    return $null
}

function Pane-Text {
    $cap = (P capture-pane -t $SESS -p) -join "`n"
    return ($cap -split "`n" | Where-Object { $_.Trim() }) -join " | "
}

function In-Mode { return ((P display-message -t $SESS -p '#{pane_in_mode}') -join '').Trim() }

function Window-Name { return ((P display-message -t $SESS -p '#{window_name}') -join '').Trim() }

function Ensure-CopyMode {
    if ((In-Mode) -ne "1") {
        P copy-mode -t $SESS | Out-Null
        Start-Sleep -Milliseconds 500
    }
}

# Open the Z prompt, type `text`, press `keys`, read the prompt, close it.
function Edit-After($text, $keys) {
    Ensure-CopyMode
    Inj "Z"
    Inj $text
    if ($keys) { Inj $keys }
    $s = Prompt-State "(edit)"
    Inj "{ESC}"
    return $s
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

# -- 1. editing keys --

Write-Head "1. editing keys at a prompt opened by a copy mode binding"
$cases = @(
    @{ Label = "abc (typing only)";          Text = "abc";     Keys = "";                      Want = "abc"   },
    @{ Label = "abc, C-u, x";                Text = "abc";     Keys = "^ux";                   Want = "x"     },
    @{ Label = "abc, Left Left, X";          Text = "abc";     Keys = "{LEFT}{LEFT}X";         Want = "aXbc"  },
    @{ Label = "bc, C-a, a, C-e, d";         Text = "bc";      Keys = "^aa^ed";                Want = "abcd"  },
    @{ Label = "ac, Home Right, b, End, d";  Text = "ac";      Keys = "{HOME}{RIGHT}b{END}d";  Want = "abcd"  },
    @{ Label = "abXc, Left, BSpace";         Text = "abXc";    Keys = "{LEFT}$BSPACE";         Want = "abc"   },
    @{ Label = "abXc, Left Left, Delete";    Text = "abXc";    Keys = "{LEFT}{LEFT}$DELETE";   Want = "abc"   },
    @{ Label = "abc def, Left Left, C-k";    Text = "abc def"; Keys = "{LEFT}{LEFT}^k";        Want = "abc d" },
    @{ Label = "abc def, C-w, X";            Text = "abc def"; Keys = "^wX";                   Want = "abc X" },
    @{ Label = "abc, Left, C-t";             Text = "abc";     Keys = "{LEFT}^t";              Want = "acb"   },
    @{ Label = "abc, Up (no history)";       Text = "abc";     Keys = "{UP}";                  Want = "abc"   }
)
foreach ($c in $cases) {
    $s = Edit-After $c.Text $c.Keys
    $v = if ($s) { $s.Text } else { "<no prompt>" }
    Check ($v -ceq $c.Want) "$($c.Label): [$v]" "$($c.Label): wanted [$($c.Want)], the prompt reads [$v]"
}

# -- 2. the cursor is drawn where it is --

Write-Head "2. the cursor is drawn where it is"
Ensure-CopyMode
Inj "Z"
Inj "abcd{LEFT}{LEFT}"
$s = Prompt-State "(edit)"
$col = if ($s) { $s.Cursor } else { $null }
Check ($col -eq 2) "abcd, Left Left: the cursor is on column 2 of the text" "abcd, Left Left: the cursor is on column [$col] of the text, wanted 2"
Inj "{ESC}"

# -- 3. the command runs on the whole line --

Write-Head "3. a line edited mid way runs as edited"
Ensure-CopyMode
Inj "Y"
Inj "TST{LEFT}{LEFT}E"
$s = Prompt-State "(name)"
$v = if ($s) { $s.Text } else { "<no prompt>" }
Check ($v -ceq "TEST") "the prompt reads [$v]" "the prompt reads [$v]"
Inj "{ENTER}"
Start-Sleep -Milliseconds 500
$w = Window-Name
Check ($w -ceq "TEST") "the window is TEST" "the window reads [$w]"

# -- 4. -T search shares the copy mode search history --

Write-Head "4. a -T search prompt shares the search history"
Ensure-CopyMode
Inj "/"
Inj "line{ENTER}"
Ensure-CopyMode
Inj "S"
Inj "{UP}"
$s = Prompt-State "(hist)"
$v = if ($s) { $s.Text } else { "<no prompt>" }
Check ($v -ceq "line") "after / line, S then Up: [$v]" "after / line, S then Up: wanted [line], the prompt reads [$v]"
Inj "{ESC}"
Ensure-CopyMode
Inj "S"
Inj "MARK{ENTER}"
Ensure-CopyMode
Inj "/"
Inj "{UP}"
$s = Prompt-State "(search down)"
$v = if ($s) { $s.Text } else { "<no prompt>" }
Check ($v -ceq "MARK") "after S MARK, / then Up: [$v]" "after S MARK, / then Up: wanted [MARK], the prompt reads [$v]"
Inj "{ESC}"
Ensure-CopyMode
Inj "S"
Inj "{UP}{UP}"
$s = Prompt-State "(hist)"; $v1 = if ($s) { $s.Text } else { "<no prompt>" }
Inj "{DOWN}"
$s = Prompt-State "(hist)"; $v2 = if ($s) { $s.Text } else { "<no prompt>" }
Inj "{DOWN}"
$s = Prompt-State "(hist)"; $v3 = if ($s) { $s.Text } else { "<no prompt>" }
$walk = "$v1|$v2|$v3"
Check ($walk -ceq "line|MARK|") "S, Up Up, Down, Down: [$walk]" "S, Up Up, Down, Down: wanted [line|MARK|], read [$walk]"
Inj "{ESC}"

# -- 5. what must not change --

Write-Head "5. the pane stays in copy mode and gets none of the keys"
Ensure-CopyMode
Inj "Z"
Inj "abc{LEFT}"
Inj "{ESC}"
Check ((In-Mode) -eq "1") "Escape closes the prompt and copy mode stays" "Escape left copy mode"
Inj "q"
Start-Sleep -Milliseconds 300
if ((In-Mode) -ne "0") { P send-keys -t $SESS -X cancel | Out-Null }
$after = Pane-Text
Check ($after -eq $script:paneBefore) "the pane is unchanged" "the pane changed: [$after]"

# -- Done --

Kill-Rig
Remove-Item $TMP -Recurse -Force -EA SilentlyContinue

Write-Host ""
Write-Host "passed: $script:TestsPassed  failed: $script:TestsFailed" -ForegroundColor Cyan
if ($script:TestsFailed -gt 0) { exit 1 }
exit 0
