# PR #873: #{history_size}, #{copy_cursor_x}, #{copy_cursor_y} and
# #{scroll_position} must answer for the TARGET pane, as in tmux, where
# history_size is the pane's own grid (format.c format_cb_history_size) and
# the copy formats come from that pane's own mode (window-copy.c
# window_copy_formats, added per pane by format_defaults_pane).
#
# Before the PR, on two panes where %a holds 174 history lines and %b none:
#
#   b active, no copy mode    list-panes  %a hist=0   %b hist=0
#   %a in copy mode, b active list-panes  %a cx=31 cy=29 sp=10
#                                         %b cx=31 cy=29 sp=10 (not in mode)
#
# tmux 3.8 for the same steps: %a hist=172 and its own copy cursor, %b hist=0
# with no copy values. psmux keeps answering 0 rather than an empty string
# for a pane outside copy mode, as it did before; both read false in #{?}.
#
# Set PSMUX_TEST_EXE to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$script:TestsPassed = 0; $script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }
Write-Host "binary: $PSMUX" -ForegroundColor Cyan

foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE') { Remove-Item "env:$v" -EA SilentlyContinue }
$NS   = "pr873_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$SESS = "fmt"
function P { & $PSMUX -L $NS @args 2>&1 }
$F = 'hist=#{history_size} mode=#{pane_in_mode} cx=#{copy_cursor_x} cy=#{copy_cursor_y} sp=#{scroll_position}'
function D($t) { return [string](P display-message -t $t -p $F) }
function Val($s, $k) { if ($s -match "\b$k=(\d+)") { return [int]$Matches[1] } return -1 }
function Cleanup { P kill-server | Out-Null; Start-Sleep -Milliseconds 500; Get-ChildItem "$psmuxDir\${NS}__*" -EA SilentlyContinue | Remove-Item -Force -EA SilentlyContinue }

P new-session -d -s $SESS -x 120 -y 30 | Out-Null
Start-Sleep -Seconds 3
P split-window -h -t $SESS | Out-Null
Start-Sleep -Seconds 2
$ids = @(P list-panes -t $SESS -F '#{pane_id}')
if ($ids.Count -ne 2) { Write-Fail "expected two panes, got [$($ids -join ' ')]"; Cleanup; exit 1 }
$a = [string]$ids[0]; $b = [string]$ids[1]
P send-keys -t $a '1..200 | % { "line $_" }' Enter | Out-Null
for ($i = 0; $i -lt 40 -and (Val (D $a) 'hist') -lt 150; $i++) { Start-Sleep -Milliseconds 250 }
P select-pane -t $b | Out-Null
Start-Sleep -Milliseconds 300

Write-Head "1. history_size of a pane that is not the active one"
$da = D $a; $db = D $b
Check ((Val $da 'hist') -ge 150) "display -t $a gives [$da]" "display -t $a gives [$da]"
Check ((Val $db 'hist') -lt 50) "display -t $b gives [$db]" "display -t $b gives [$db]"
$lp = @(P list-panes -t $SESS -F "#{pane_id} $F")
$la = [string]($lp | Where-Object { $_ -like "$a *" }); $lb = [string]($lp | Where-Object { $_ -like "$b *" })
Check ((Val $la 'hist') -ge 150 -and (Val $lb 'hist') -lt 50) "list-panes: [$la] [$lb]" "list-panes: [$la] [$lb]"

Write-Head "2. copy formats of a pane parked in copy mode, and of one outside it"
P select-pane -t $a | Out-Null
P copy-mode -t $a | Out-Null
Start-Sleep -Milliseconds 400
P send-keys -t $a -X -N 10 scroll-up | Out-Null
Start-Sleep -Milliseconds 400
$inA = D $a
P select-pane -t $b | Out-Null
Start-Sleep -Milliseconds 400
$da = D $a; $db = D $b
Check ((Val $da 'mode') -eq 1 -and (Val $da 'sp') -eq (Val $inA 'sp') -and (Val $da 'sp') -gt 0 -and (Val $da 'cx') -eq (Val $inA 'cx') -and (Val $da 'cy') -eq (Val $inA 'cy')) "parked $a keeps its own values [$da]" "parked ${a}, before [$inA] after [$da]"
Check ((Val $db 'mode') -eq 0 -and (Val $db 'sp') -eq 0 -and (Val $db 'cx') -eq 0 -and (Val $db 'cy') -eq 0) "active $b outside copy mode reads [$db]" "active $b outside copy mode reads [$db]"

Write-Head "3. from another window"
P new-window -t $SESS | Out-Null
Start-Sleep -Seconds 2
$da = D "${SESS}:0.0"; $db = D "${SESS}:0.1"
Check ((Val $da 'hist') -ge 150 -and (Val $da 'mode') -eq 1 -and (Val $da 'sp') -gt 0) "${SESS}:0.0 gives [$da]" "${SESS}:0.0 gives [$da]"
Check ((Val $db 'hist') -lt 50 -and (Val $db 'sp') -eq 0 -and (Val $db 'cx') -eq 0) "${SESS}:0.1 gives [$db]" "${SESS}:0.1 gives [$db]"
$cur = [string](P display-message -p $F)
Check ((Val $cur 'hist') -lt 50 -and (Val $cur 'sp') -eq 0 -and (Val $cur 'mode') -eq 0) "the new window's pane gives [$cur]" "the new window's pane gives [$cur]"

Cleanup
Write-Host ""
Write-Host "=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
