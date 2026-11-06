# send-keys -R resets the pane's terminal state and sends the program nothing,
# as tmux does (cmd-send-keys.c: colour_palette_clear, input_reset(wp->ictx, 1),
# and with no keys, return). Keys after -R are still sent.
#
# psmux used to turn -R into a key named `__RESET__`, so the shell received the
# text `__RESET__`, and the server took -R for a flag with a value, so in
# `send-keys -R -t P Enter` style lists the word after -R was dropped.
#
# [1] the inbox conhost route: nothing typed, keys after -R delivered.
# [2] OpenConsole (PSMUX_CONPTY_DIR_FOR_TEST), where psmux reads the program's
#     own bytes: the same, and the mouse mode and the line drawing set a
#     program left on are off again after -R. (The inbox conhost keeps both to
#     itself, so psmux's parser never sees them on that route.)
#
# Run: pwsh -NoProfile -File tests\test_send_keys_reset.ps1
# PSMUX_TEST_BIN overrides the binary under test.

$ErrorActionPreference = 'Continue'
$env:SHELL = $null
Remove-Item Env:PSMUX_CONPTY_DIR -ErrorAction SilentlyContinue
$ExePath = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -ErrorAction Stop).Source }
$ConptyDir = $env:PSMUX_CONPTY_DIR_FOR_TEST
$DataDir = Join-Path ([IO.Path]::GetTempPath()) "psmux-sendkeysR-$PID"
New-Item -ItemType Directory -Force $DataDir | Out-Null
$env:PSMUX_DATA_DIR = $DataDir
$script:Passed = 0
$script:Failed = 0
$script:NS = $null

function Check {
    param([bool]$Condition, [string]$Name, [string]$Detail = '')
    if ($Condition) { $script:Passed++; Write-Output "  [PASS] $Name" } else { $script:Failed++; Write-Output "  [FAIL] $Name $Detail" }
}
function P { & $ExePath -L $script:NS @args 2>&1 }
function Screen { (P capture-pane -p -t r) -join "`n" }
function Wait-Screen {
    param([string]$Pattern, [int]$TimeoutMs = 15000)
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        if ((Screen) -match $Pattern) { return $true }
        Start-Sleep -Milliseconds 200
    }
    (Screen) -match $Pattern
}
function Fmt { param([string]$F) ((P display-message -p -t r $F) -join '').Trim() }
# A pwsh line that writes ESC followed by $Seq, without a newline.
function Emit { param([string]$Seq) "[Console]::Write([char]27 + '$Seq')" }

function Start-Pane {
    param([string]$Label)
    $script:NS = "sendkeysR$PID$Label"
    P new-session -d -s r -x 70 -y 14 'pwsh -NoProfile -NoLogo' | Out-Null
    Check (Wait-Screen 'PS .*>') "${Label}: the pane shows a prompt"
}

function Part-Common {
    param([string]$Label)
    Start-Pane $Label

    # -R alone types nothing.
    P send-keys -t r 'clear' Enter | Out-Null
    Start-Sleep -Milliseconds 800
    P send-keys -R -t r | Out-Null
    P send-keys -t r -l 'echo AFTER_$((40+2))' | Out-Null
    P send-keys -t r Enter | Out-Null
    $ok = Wait-Screen '(?m)^AFTER_42\s*$'
    $s = Screen
    Check ($ok -and $s -notmatch 'RESET') "${Label}: send-keys -R sends the shell nothing" "(screen: $($s -replace "`n", ' / '))"

    # Keys after -R are sent.
    P send-keys -t r -l 'echo KEPT_$((40+3))' | Out-Null
    P send-keys -R -t r Enter | Out-Null
    Check (Wait-Screen '(?m)^KEPT_43\s*$') "${Label}: send-keys -R -t r Enter still sends Enter"

    Check ((P has-session -t r) -eq $null -and $LASTEXITCODE -eq 0) "${Label}: the session is still up"
}

try {
    Write-Output "[1] inbox conhost"
    Part-Common 'inbox'
    P kill-server | Out-Null

    if ($ConptyDir -and (Test-Path (Join-Path $ConptyDir 'conpty.dll'))) {
        Write-Output "[2] OpenConsole ($ConptyDir)"
        $env:PSMUX_CONPTY_DIR = $ConptyDir
        try {
            Part-Common 'oc'
            # The program's mouse mode is part of the state -R resets (tmux
            # screen_write_reset sets the modes back to cursor and wrap).
            P send-keys -t r (Emit '[?1000h') Enter | Out-Null
            $on = $false
            for ($i = 0; $i -lt 40 -and -not $on; $i++) { Start-Sleep -Milliseconds 200; $on = (Fmt '#{mouse_any_flag}') -eq '1' }
            Check $on "oc: the program turned mouse mode on (mouse_any_flag 1)" "(got '$(Fmt '#{mouse_any_flag}')')"
            P send-keys -R -t r | Out-Null
            Start-Sleep -Milliseconds 500
            Check ((Fmt '#{mouse_any_flag}') -eq '0') "oc: after send-keys -R mouse_any_flag is 0" "(got '$(Fmt '#{mouse_any_flag}')')"
            # The line drawing set left on by a program is ASCII again after -R.
            P send-keys -t r 'clear' Enter | Out-Null
            Start-Sleep -Milliseconds 800
            $line = (Emit '(0') + "; 'SET:lqqk'"
            P send-keys -t r $line Enter | Out-Null
            Check (Wait-Screen 'SET:┌──┐') "oc: ESC ( 0 draws lqqk as a box first"
            P send-keys -R -t r | Out-Null
            Start-Sleep -Milliseconds 300
            # Bytes the program writes after the reset, with no designation of
            # its own: psmux must read them as ASCII.
            P send-keys -t r "'PLAIN:' + 'lqqk'" Enter | Out-Null
            $plain = Wait-Screen 'PLAIN:lqqk'
            Check $plain "oc: after send-keys -R, lqqk prints as letters" "(screen: $((Screen) -replace "`n", ' / '))"
        }
        finally { Remove-Item Env:PSMUX_CONPTY_DIR -ErrorAction SilentlyContinue }
    } else {
        Write-Output "[2] OpenConsole: SKIP (set PSMUX_CONPTY_DIR_FOR_TEST to a folder with conpty.dll)"
    }
}
finally {
    if ($script:NS) { P kill-server | Out-Null }
    Start-Sleep -Milliseconds 300
    Remove-Item -Recurse -Force $DataDir -ErrorAction SilentlyContinue
}

Write-Output "=== Results: $script:Passed passed, $script:Failed failed ==="
exit $script:Failed
