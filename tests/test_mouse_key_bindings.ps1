# tmux mouse key bindings for the wheel over a pane.
#
# psmux used to accept `bind -n WheelUpPane ...`, `bind -n C-WheelUpPane ...`
# and `unbind -n WheelUpPane` without a word and then ignore them: the binding
# never fired and list-keys did not show it. Measured side by side in Windows
# Terminal against tmux 3.4 (WSL) with real wheel input, before this change:
#
#   step                                   tmux 3.4              psmux
#   list-keys | grep Wheel                 root + copy-mode      nothing
#   bind -n WheelUpPane set -g @r1 yes     @r1=yes, no copy mode @r1 unset, copy mode
#   bind -n C-WheelUpPane set -g @r4 yes   @r4=yes               @r4 unset
#   unbind -n WheelUpPane, wheel up        nothing               copy mode
#   copy-mode, bind -T copy-mode WheelUp.. fires                 ignored
#   Ctrl+wheel at a shell, nothing bound   nothing               copy mode (Ctrl dropped)
#   Ctrl+wheel over a mouse app            \e[<80;..M            \e[<64;..M
#
# This suite drives the same path a client does: the attached client turns a
# wheel notch over a pane into `pane-scroll <pane> up|down <col> <row> <mask>`
# (mask = tmux's mouse modifiers: shift 4, meta 8, ctrl 16) on its connection,
# so the lines go over the server's TCP port exactly like that.
#
# GROUND TRUTH: `display-message` (pane_in_mode, scroll_position, pane_id,
# window_zoomed_flag), `show-options` for the @ options a binding sets, and
# list-keys.
#
# Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0; $script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

$env:PSMUX_SESSION_NAME = $null; $env:PSMUX_SESSION = $null; $env:PSMUX_PANE = $null
$env:TMUX = $null; $env:TMUX_PANE = $null

# A private data dir unless the runner already isolates one.
$ownData = $false
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path ([IO.Path]::GetTempPath()) ("psmux_mkb_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
    $ownData = $true
}
$DATA = $env:PSMUX_DATA_DIR
$NS   = "mkb-" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$SESS = "mkb"

function P { & $PSMUX -L $NS @args 2>&1 }
function Wire([string]$line, [switch]$Fast) {
    $port = [int](Get-Content (Join-Path $DATA "${NS}__${SESS}.port") -Raw).Trim()
    $key  = (Get-Content (Join-Path $DATA "${NS}__${SESS}.key") -Raw).Trim()
    $c = New-Object System.Net.Sockets.TcpClient('127.0.0.1', $port)
    try {
        $s = $c.GetStream()
        $b = [Text.Encoding]::UTF8.GetBytes("AUTH $key`n$line`n")
        $s.Write($b, 0, $b.Length); $s.Flush()
        $s.ReadTimeout = 500
        $buf = New-Object byte[] 256
        try { [void]$s.Read($buf, 0, 256) } catch {}
    } finally { $c.Close() }
    if (-not $Fast) { Start-Sleep -Milliseconds 250 }
}
function PaneId { ((P display -p '#{pane_id}') -join '').TrimStart('%') }
function Wheel([string]$dir, [int]$mask = 0, [string]$pane = $null) {
    if (-not $pane) { $pane = PaneId }
    Wire "pane-scroll $pane $dir 5 5 $mask"
}
function St { (P display -p '#{pane_in_mode} #{scroll_position}') -join '' }
function Opt($n) { ((P show -gv $n) -join '').Trim() }
function Cancel { P send-keys -X cancel | Out-Null; Start-Sleep -Milliseconds 250 }
function Keys { P list-keys | Out-String }

# Copies in this suite reach the Windows clipboard: keep the user's.
$suiteClip = Get-Clipboard -Raw
try {
    P new-session -d -s $SESS -x 120 -y 30 | Out-Null
    Start-Sleep 3
    P send-keys -t $SESS '1..300 | % { "line $_" }' Enter | Out-Null
    Start-Sleep 3

    Write-Head "defaults: list-keys and the built-in wheel"
    $k = Keys
    Check ($k -match "bind-key -T root WheelUpPane ") "root WheelUpPane default listed" "root WheelUpPane missing from list-keys:`n$k"
    Check ($k -match "bind-key -T copy-mode WheelUpPane " -and $k -match "bind-key -T copy-mode-vi WheelDownPane ") "copy-mode wheel defaults listed" "copy-mode wheel defaults missing"
    Check (-not ($k -match "-T root WheelDownPane")) "no root WheelDownPane, as in tmux" "root WheelDownPane listed"
    Wheel up; $a = St; Wheel up; $b = St; Wheel down; Wheel down; $c = St
    Check ($a -eq "1 3") "wheel up enters copy mode and scrolls 3" "wheel up gave [$a]"
    Check ($b -eq "1 6") "second notch scrolls 3 more" "second notch gave [$b]"
    Check ($c -match '^0') "wheel down back to the bottom leaves copy mode" "wheel down gave [$c]"
    Cancel

    Write-Head "unbound modified wheel: tmux drops it at a shell"
    Wheel up 16; $a = St
    Check ($a -match '^0') "C-WheelUpPane unbound does not enter copy mode" "Ctrl+wheel gave [$a]"
    Cancel

    Write-Head "user bindings fire and show in list-keys"
    P bind -n WheelUpPane set -g '@r1' yes | Out-Null
    P bind -n C-WheelUpPane set -g '@r4' yes | Out-Null
    Wheel up; $a = St
    Check ((Opt '@r1') -eq 'yes' -and $a -match '^0') "bind -n WheelUpPane runs and replaces the default" "@r1=[$(Opt '@r1')] state [$a]"
    Wheel up 16
    Check ((Opt '@r4') -eq 'yes') "bind -n C-WheelUpPane runs on Ctrl+wheel" "@r4=[$(Opt '@r4')]"
    $k = Keys
    Check ($k -match "-T root WheelUpPane set -g @r1 yes" -and $k -match "-T root C-WheelUpPane set -g @r4 yes") "list-keys shows the user bindings" "list-keys:`n$k"

    Write-Head "unbind restores tmux semantics"
    P unbind -n WheelUpPane | Out-Null
    Wheel up; Wheel up; $a = St
    Check ($a -match '^0') "after unbind -n WheelUpPane the wheel no longer enters copy mode" "state [$a]"
    Check (-not ((Keys) -match "-T root WheelUpPane ")) "list-keys drops it" "still listed"

    Write-Head "copy-mode tables"
    P set -g mode-keys vi | Out-Null
    P bind -T copy-mode-vi WheelUpPane send -N1 -X scroll-up | Out-Null
    P copy-mode | Out-Null; Start-Sleep -Milliseconds 300
    Wheel up; Wheel up; $a = St
    Check ($a -eq "1 2") "bind -T copy-mode-vi WheelUpPane (scroll 1) is used in copy mode" "state [$a]"
    Wheel up 16; $b = St
    Check ($b -eq "1 2") "Ctrl+wheel in copy mode does nothing (not bound in copy-mode-vi nor root)" "state [$b]"
    P unbind -T copy-mode-vi WheelUpPane | Out-Null
    Wheel up; $c = St
    Check ($c -eq "1 2") "unbound copy-mode-vi WheelUpPane does nothing" "state [$c]"
    Cancel

    Write-Head "the mouse pane is the -t = target"
    P split-window -v | Out-Null; Start-Sleep 2
    $bottom = PaneId
    P select-pane -U | Out-Null; Start-Sleep -Milliseconds 300
    $top = PaneId
    P bind -n WheelDownPane resize-pane -Z -t = | Out-Null
    Wheel down 0 $bottom
    $z = (P display -p '#{pane_id} #{window_zoomed_flag}') -join ''
    Check ($top -ne $bottom -and $z -eq "%$bottom 1") "resize-pane -Z -t = zooms the pane under the mouse" "top %$top bottom %$bottom got [$z]"
    Write-Head "status line wheel: previous-window / next-window"
    P new-window | Out-Null; P new-window | Out-Null; Start-Sleep 1
    P select-window -t 1 | Out-Null; Start-Sleep -Milliseconds 300
    $w0 = (P display -p '#{window_index}') -join ''
    Wire "scroll-up 5 29 0 S"; $w1 = (P display -p '#{window_index}') -join ''
    Wire "scroll-down 5 29 0 S"; $w2 = (P display -p '#{window_index}') -join ''
    Check ($w0 -eq '1' -and $w1 -eq '0' -and $w2 -eq '1') "WheelUpStatus / WheelDownStatus switch windows" "windows $w0 -> $w1 -> $w2"
    Check ((Keys) -match "-T root WheelUpStatus previous-window") "list-keys shows WheelUpStatus" "WheelUpStatus missing"
    P unbind -n WheelUpStatus | Out-Null
    Wire "scroll-up 5 29 0 S"; $w3 = (P display -p '#{window_index}') -join ''
    Check ($w3 -eq '1') "unbound WheelUpStatus does nothing" "window $w3"
    P select-window -t 0 | Out-Null; Start-Sleep -Milliseconds 300

    Write-Head "end of a copy-mode drag: MouseDragEnd1Pane"
    P set -g mode-keys emacs | Out-Null
    $pid0 = ((P display -p '#{pane_id}') -join '').TrimStart('%')
    function Drag {
        P copy-mode | Out-Null; Start-Sleep -Milliseconds 300
        Wire "pane-mouse $pid0 0 2 3 M"; Wire "pane-mouse $pid0 32 10 5 M"; Wire "pane-mouse $pid0 0 10 5 m"
        (P display -p '#{pane_in_mode} #{selection_present}') -join ''
    }
    P set-buffer MARK | Out-Null
    $a = Drag
    Check ($a -match '^0' -and ((P show-buffer) -join '') -ne 'MARK') "default: copied and left copy mode" "state [$a] buffer [$((P show-buffer) -join '')]"
    P unbind -T copy-mode MouseDragEnd1Pane | Out-Null
    P set-buffer MARK | Out-Null
    $b = Drag
    Check ($b -eq '1 1' -and ((P show-buffer) -join '') -eq 'MARK') "unbound: the selection stays, nothing copied" "state [$b] buffer [$((P show-buffer) -join '')]"
    Cancel
    P bind -T copy-mode MouseDragEnd1Pane send -X copy-selection-and-cancel | Out-Null
    P set-buffer MARK | Out-Null
    $c2 = Drag
    Check ($c2 -match '^0' -and ((P show-buffer) -join '') -ne 'MARK') "a user MouseDragEnd1Pane binding runs" "state [$c2] buffer [$((P show-buffer) -join '')]"

    Write-Head "brace blocks in a binding (source-file)"
    $conf = Join-Path $DATA "mkb_braces.conf"
    [IO.File]::WriteAllText($conf, "bind -n F8 { set -g @b1 yes; set -g @b2 yes }`nbind -n F9 {`n  set -g @c1 yes`n  set -g @c2 yes`n}`nbind -n F10 { if -F '#{pane_in_mode}' { set -g @d in } { set -g @d out } }`n")
    $out = (P source-file $conf) -join ' '
    $k = Keys
    Check ($k -match "-T root F8 set -g @b1 yes \\; set -g @b2 yes") "one line block becomes a command list" "F8 line missing:`n$k"
    Check ($k -match "-T root F9 set -g @c1 yes \\; set -g @c2 yes") "multi line block becomes a command list" "F9 line missing"
    Check ($k -match "-T root F10 if -F '#\{pane_in_mode\}' \{ set -g @d in \} \{ set -g @d out \}") "nested blocks stay one command" "F10 line missing"
    Check (-not ($out -match 'unknown command')) "no unknown command warnings" "warnings: $out"

    Write-Head "refresh-client -l reads the Windows clipboard into a buffer (#796)"
    $savedClip = Get-Clipboard -Raw
    try {
        Set-Clipboard -Value "MKBCLIP796"
        P refresh-client -l | Out-Null
        $top = (P show-buffer) -join ''
        Check ($top -eq 'MKBCLIP796') "refresh-client -l makes the clipboard the top buffer" "top buffer [$top]"
    } finally { if ($null -ne $savedClip) { Set-Clipboard -Value $savedClip } }

    Write-Head "double and triple click (tmux click timer)"
    P select-window -t 0 | Out-Null; Start-Sleep -Milliseconds 300
    P send-keys -t $SESS 'cls; 1..20 | % { "alpha$_ beta$_ gamma$_" }' Enter | Out-Null
    Start-Sleep 3
    $pid1 = ((P display -p '#{pane_id}') -join '').TrimStart('%')
    # Presses well inside the 300 ms double click window.
    function Press { Wire "pane-mouse $pid1 0 9 5 M" -Fast; Wire "pane-mouse $pid1 0 9 5 m" -Fast }
    P set-buffer MARK | Out-Null
    Press; Press; Start-Sleep -Milliseconds 900
    $w = (P show-buffer) -join ''
    Check ($w -match '^beta\d+$') "double click selects and copies the word" "buffer [$w]"
    P set-buffer MARK | Out-Null
    Press; Press; Press; Start-Sleep -Milliseconds 900
    $l = ((P show-buffer) -join ' ').Trim()
    Check ($l -match '^alpha\d+ beta\d+ gamma\d+$') "triple click selects and copies the line" "buffer [$l]"
    P bind -n DoubleClick1Pane set -g '@dc' hit | Out-Null
    P set-buffer MARK | Out-Null
    Press; Press; Start-Sleep -Milliseconds 900
    Check ((Opt '@dc') -eq 'hit' -and ((P show-buffer) -join '') -eq 'MARK') "a DoubleClick1Pane binding runs instead" "@dc=[$(Opt '@dc')] buffer [$((P show-buffer) -join '')]"
} finally {
    if ($null -ne $suiteClip) { Set-Clipboard -Value $suiteClip }
    P kill-server | Out-Null
    Start-Sleep 1
    if ($ownData) { Remove-Item -Recurse -Force $DATA -EA SilentlyContinue }
}

Write-Host "`nResults: $script:TestsPassed passed, $script:TestsFailed failed"
exit $(if ($script:TestsFailed -eq 0) { 0 } else { 1 })
