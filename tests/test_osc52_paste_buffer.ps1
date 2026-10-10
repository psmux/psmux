# OSC 52 paste_add parity: tmux input_osc_52 (input.c) adds the decoded
# payload to the paste buffer stack via paste_add IN ADDITION to forwarding
# it to the host terminal. psmux only forwarded, so show-buffer and
# paste-buffer never saw pane initiated OSC 52 content. This test proves the
# buffer side. Critically it runs DETACHED with no dump-state polling at all:
# tmux captures the buffer server side during input parsing whether or not a
# client is attached, so psmux must too.
#
# tmux takes the payload into a buffer only when set-clipboard is `on`
# (input.c input_osc_52_parse returns unless the value is 2), and the default
# is `external` (#771), so this suite turns `on` before it emits anything.
# tests\test_issue771_set_clipboard.ps1 covers `off` and `external`.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$SESSION = "osc52buf"
$psmuxDir = "$env:USERPROFILE\.psmux"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

function Cleanup {
    & $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue
}

function Emit-Osc52 {
    param([string]$B64Payload)
    $cmd = "Write-Host -NoNewline (([char]27)+']52;c;$B64Payload'+([char]7))"
    & $PSMUX send-keys -t $SESSION $cmd Enter 2>&1 | Out-Null
}

Cleanup
& $PSMUX new-session -d -s $SESSION
Start-Sleep -Seconds 3
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) { Write-Fail "Session creation failed"; exit 1 }
& $PSMUX set-option -s set-clipboard on 2>&1 | Out-Null
$ready = $false
for ($i = 0; $i -lt 60; $i++) {
    $cap = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($cap -match "PS [A-Z]:\\") { $ready = $true; break }
    Start-Sleep -Milliseconds 250
}
if (-not $ready) { Write-Fail "Shell prompt never appeared"; Cleanup; exit 1 }

Write-Host "`n=== OSC 52 paste buffer parity (tmux paste_add) ===" -ForegroundColor Cyan

# === Test 1: detached OSC 52 lands in the paste buffer stack ===
Write-Host "`n[1] OSC 52 in a fully detached session sets a paste buffer" -ForegroundColor Yellow
Emit-Osc52 "Zm9vYmFy"   # "foobar"
Start-Sleep -Seconds 2
$buf = (& $PSMUX show-buffer -t $SESSION 2>&1 | Out-String).Trim()
if ($buf -eq "foobar") { Write-Pass "show-buffer returns 'foobar' with no client ever attached" }
else { Write-Fail "expected 'foobar', show-buffer returned '$buf'" }

# === Test 2: a second OSC 52 stacks a new buffer on top ===
Write-Host "`n[2] second OSC 52 pushes a new buffer on top of the stack" -ForegroundColor Yellow
Emit-Osc52 "aGVsbG8gd29ybGQ="   # "hello world"
Start-Sleep -Seconds 2
$buf = (& $PSMUX show-buffer -t $SESSION 2>&1 | Out-String).Trim()
if ($buf -eq "hello world") { Write-Pass "newest payload is on top of the stack" }
else { Write-Fail "expected 'hello world' on top, got '$buf'" }
$list = (& $PSMUX list-buffers -t $SESSION 2>&1 | Out-String).Trim()
$count = ($list -split "`n" | Where-Object { $_.Trim() }).Count
if ($count -ge 2) { Write-Pass "list-buffers shows $count buffers (stack, tmux parity)" }
else { Write-Fail "expected at least 2 buffers, list-buffers shows: '$list'" }

# === Test 3: invalid base64 leaves the stack untouched ===
Write-Host "`n[3] invalid base64 does not touch the buffer stack" -ForegroundColor Yellow
Emit-Osc52 "bad*payload"
Start-Sleep -Seconds 2
$buf = (& $PSMUX show-buffer -t $SESSION 2>&1 | Out-String).Trim()
if ($buf -eq "hello world") { Write-Pass "top buffer still 'hello world' after invalid payload" }
else { Write-Fail "buffer corrupted by invalid payload: '$buf'" }

# === Test 4: whitespace payload decodes into the buffer (tmux b64_pton) ===
Write-Host "`n[4] whitespace wrapped payload lands in the buffer" -ForegroundColor Yellow
Emit-Osc52 "d3JhcCBtZQ ="   # "wrap me" with a space before the padding
Start-Sleep -Seconds 2
$buf = (& $PSMUX show-buffer -t $SESSION 2>&1 | Out-String).Trim()
if ($buf -eq "wrap me") { Write-Pass "whitespace payload decoded into buffer" }
else { Write-Fail "expected 'wrap me', got '$buf'" }

# === Test 5: paste-buffer types the captured content back into the pane ===
Write-Host "`n[5] paste-buffer round trips the captured content" -ForegroundColor Yellow
& $PSMUX send-keys -t $SESSION "Write-Host PRE_PASTE_MARKER" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 1
& $PSMUX paste-buffer -t $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 2
$cap = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
if ($cap -match "wrap me") { Write-Pass "pasted buffer content appeared in the pane" }
else { Write-Fail "buffer content did not paste back into the pane" }

# === Test 6: attached client forwarding still works after buffer capture ===
Write-Host "`n[6] client forwarding still delivers clipboard_osc52" -ForegroundColor Yellow
# Clear whatever test 5's paste left on the prompt line so the next emission
# actually executes (Escape wipes the PSReadLine input line).
& $PSMUX send-keys -t $SESSION Escape 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
Emit-Osc52 "Zm9yd2FyZA=="   # "forward"
Start-Sleep -Seconds 2
$port = (Get-Content "$psmuxDir\$SESSION.port" -Raw).Trim()
$key = (Get-Content "$psmuxDir\$SESSION.key" -Raw).Trim()
$tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
$tcp.NoDelay = $true; $tcp.ReceiveTimeout = 5000
$stream = $tcp.GetStream()
$writer = [System.IO.StreamWriter]::new($stream)
$reader = [System.IO.StreamReader]::new($stream)
$writer.Write("AUTH $key`n"); $writer.Flush(); $null = $reader.ReadLine()
$writer.Write("PERSISTENT`n"); $writer.Flush()
$writer.Write("dump-state`n"); $writer.Flush()
$best = $null
for ($j = 0; $j -lt 50; $j++) {
    try { $line = $reader.ReadLine() } catch { break }
    if ($null -eq $line) { break }
    if ($line -ne "NC" -and $line.Length -gt 100) { $best = $line; break }
}
$tcp.Close()
$fwd = "FIELD_ABSENT"
if ($best -and $best -match '"clipboard_osc52":"([^"]*)"') {
    $b64 = $Matches[1]
    if ($b64.Length % 4 -ne 0) { $b64 += "=" * (4 - ($b64.Length % 4)) }
    try { $fwd = [System.Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($b64)) } catch { $fwd = "DECODE_ERROR" }
}
if ($fwd -eq "forward") { Write-Pass "clipboard_osc52 still delivered to polling client" }
else { Write-Fail "expected 'forward' in clipboard_osc52, got '$fwd'" }
$buf = (& $PSMUX show-buffer -t $SESSION 2>&1 | Out-String).Trim()
if ($buf -eq "forward") { Write-Pass "same payload also captured in the buffer stack" }
else { Write-Fail "expected 'forward' in buffer, got '$buf'" }

Cleanup

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
