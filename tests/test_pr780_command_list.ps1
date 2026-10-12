# PR #780 and its follow up: a command list on the command line
# (`psmux cmd1 \; cmd2`) runs every command, in order, and stops at the first
# that fails, as tmux's cmd-queue.c does.
#
# Before: the CLI dispatched on the first command and dropped or folded the
# rest (`select-pane -t p \; set @c x` never set @c, `display -p one \;
# display -p two` printed `one \; display-message two`); the server's
# one-shot loop skipped every other command of a list of three or more and
# ended the list at the first output command; a failed command did not stop
# the list. After the PR, commands naming ANOTHER session went down the first
# session's connection and acted on it (`send-keys -t y` typed into x), `ls`
# listed one session and `kill-server` ended only one; `new-session -d -s x
# \; ...` created no session at all.
#
# Isolation: a unique -L namespace only; cleanup is `-L <ns> kill-server`
# and the client window this script opened, by PID.
#
# Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$script:TestsPassed = 0; $script:TestsFailed = 0
$script:Opened = @()

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan
foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX_TARGET_SESSION','PSMUX_TARGET_FULL') { Remove-Item "Env:\$v" -EA SilentlyContinue }

$NS  = "a780_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$SEP = [char]92 + ";"   # a literal \; argv token
function P { & $PSMUX -L $NS @args 2>&1 | ForEach-Object { "$_" } }
function Run { $o = P @args; [pscustomobject]@{ Rc = $LASTEXITCODE; Out = (($o) -join "`n").Trim() } }
function Opt($t, $name) { ((P show-options -v -t $t $name) -join '').Trim() }
function Cap($t) { (P capture-pane -t $t -p) -join "`n" }

$defaultBefore = ((& $PSMUX ls 2>&1) -join "`n")

try {
    P new-session -d -s x -x 100 -y 30 | Out-Null
    P new-session -d -s y -x 100 -y 30 | Out-Null
    Start-Sleep -Milliseconds 600

    Write-Head "A: every command of the list runs, in order"
    P select-pane -t x $SEP set-option -t x '@c' after | Out-Null
    Check ((Opt x '@c') -eq 'after') "select-pane then set-option sets @c" "@c = '$(Opt x '@c')'"
    $r = Run display-message -t x -p one $SEP display-message -p two
    Check ($r.Rc -eq 0 -and $r.Out -eq "one`ntwo") "display -p one, display -p two print two lines" "rc=$($r.Rc) out='$($r.Out)'"
    $r = Run list-windows -t x -F 'w#{window_index}' $SEP set-option -t x '@x' y
    Check ($r.Out -eq 'w0' -and (Opt x '@x') -eq 'y') "an output command does not end the list" "out='$($r.Out)' @x='$(Opt x '@x')'"
    P set -t x '@r1' a $SEP set -t x '@r2' b $SEP set -t x '@r3' c $SEP set -t x '@r4' d | Out-Null
    $vals = (Opt x '@r1') + (Opt x '@r2') + (Opt x '@r3') + (Opt x '@r4')
    Check ($vals -eq 'abcd') "a list of four sets all four" "got '$vals'"

    Write-Head "B: raw TCP list of five (pending_chain taken once)"
    $port = [int](Get-Content "$psmuxDir\${NS}__x.port" -Raw).Trim()
    $key = (Get-Content "$psmuxDir\${NS}__x.key" -Raw).Trim()
    $c = [System.Net.Sockets.TcpClient]::new('127.0.0.1', $port)
    $st = $c.GetStream(); $w = [System.IO.StreamWriter]::new($st); $w.NewLine = "`n"
    $w.WriteLine("AUTH $key"); $w.WriteLine('set @t1 a ; set @t2 b ; set @t3 c ; list-windows -F lw ; set @t4 d'); $w.Flush()
    $c.Client.Shutdown([System.Net.Sockets.SocketShutdown]::Send)
    $rd = [System.IO.StreamReader]::new($st); $st.ReadTimeout = 5000
    $reply = try { $rd.ReadToEnd() } catch { '' }
    $c.Close()
    $vals = (Opt x '@t1') + (Opt x '@t2') + (Opt x '@t3') + (Opt x '@t4')
    Check ($vals -eq 'abcd' -and $reply -match 'lw') "all five ran (reply has list-windows)" "vals='$vals' reply='$reply'"

    Write-Head "C: a failed command ends the list, exit 1"
    $r = Run set -t x '@y1' a $SEP select-pane -t '%99' $SEP set -t x '@y2' b
    Check ($r.Rc -eq 1 -and (Opt x '@y1') -eq 'a' -and (Opt x '@y2') -eq '') "can't find pane stops the list (rc 1)" "rc=$($r.Rc) @y1='$(Opt x '@y1')' @y2='$(Opt x '@y2')'"
    $r = Run set -t x '@u1' a $SEP bogus-command $SEP set -t x '@u2' b
    Check ($r.Rc -eq 1 -and $r.Out -match 'unknown command: bogus-command' -and (Opt x '@u2') -eq '') "an unknown command stops the list" "rc=$($r.Rc) out='$($r.Out)' @u2='$(Opt x '@u2')'"
    $r = Run display-message -t x -p before $SEP select-window -t 'x:99' $SEP display-message -p after
    Check ($r.Rc -eq 1 -and $r.Out -match 'before' -and $r.Out -notmatch 'after') "output before the error is kept, nothing after" "rc=$($r.Rc) out='$($r.Out)'"

    Write-Head "D: commands naming another session reach that session"
    P set -t x '@cs1' a $SEP set -t y '@cs2' b | Out-Null
    Check ((Opt x '@cs1') -eq 'a' -and (Opt y '@cs2') -eq 'b' -and (Opt x '@cs2') -eq '') "set -t x then set -t y land in x and y" "x@cs1='$(Opt x '@cs1')' y@cs2='$(Opt y '@cs2')' x@cs2='$(Opt x '@cs2')'"
    $r = Run display-message -t x -p '#{session_name}' $SEP display-message -t y -p '#{session_name}'
    Check ($r.Out -eq "x`ny") "display -t x, display -t y print x then y" "out='$($r.Out)'"
    P send-keys -t x 'echo A780X' Enter $SEP send-keys -t y 'echo A780Y' Enter | Out-Null
    Start-Sleep -Milliseconds 1500
    Check ((Cap y).Contains('A780Y') -and -not (Cap x).Contains('A780Y')) "send-keys -t y types into y, not x" "y has it: $((Cap y).Contains('A780Y')), x has it: $((Cap x).Contains('A780Y'))"
    $ypane = ((P display-message -t y -p '#{pane_id}') -join '').Trim()
    P select-pane -t $ypane $SEP set-option -t y '@own' yes | Out-Null
    Check ((Opt y '@own') -eq 'yes') "select-pane -t $ypane (owned by y, #627) then set" "@own='$(Opt y '@own')'"

    Write-Head "E: client side commands in a list"
    $r = Run ls -F '#{session_name}' $SEP display-message -t x -p tail
    Check ($r.Out -eq "x`ny`ntail") "ls lists every session, then the next command runs" "out='$($r.Out)'"
    P run-shell 'exit 0' $SEP set -t x '@rs' after | Out-Null
    Check ((Opt x '@rs') -eq 'after') "run-shell then set" "@rs='$(Opt x '@rs')'"
    P if-shell -t x 'exit 0' 'set -t x @i1 yes' $SEP set -t x '@i2' after | Out-Null
    Check ((Opt x '@i1') -eq 'yes' -and (Opt x '@i2') -eq 'after') "if-shell then set" "@i1='$(Opt x '@i1')' @i2='$(Opt x '@i2')'"

    Write-Head "F: new-session with a list"
    $r = Run new-session -d -s n1 -P -F 'made:#{session_name}' $SEP split-window -t n1 $SEP display-message -p '#{window_panes}'
    Check ($r.Rc -eq 0 -and $r.Out -eq "made:n1`n2") "new-session -d -P, split-window, display run in order" "rc=$($r.Rc) out='$($r.Out)'"
    P new-session -d -s n2 $SEP set -g '@na' b | Out-Null
    Check ((((P show-options -gv -t n2 '@na') -join '').Trim()) -eq 'b') "new-session -d then set -g" "@na='$((P show-options -gv -t n2 '@na') -join '')'"
    P new-session -A -d -s n2 $SEP set -t n2 '@again' yes | Out-Null
    Check ((Opt n2 '@again') -eq 'yes') "new-session -A on an existing session runs the list there" "@again='$(Opt n2 '@again')'"

    Write-Head "G: data that only contains ; stays data (#499)"
    P set -t x '@k' 'a ; b' | Out-Null
    P set -t x '@k2' 'x;' | Out-Null
    Check ((Opt x '@k') -eq 'a ; b' -and (Opt x '@k2') -eq 'x;') "quoted ; values kept" "@k='$(Opt x '@k')' @k2='$(Opt x '@k2')'"
    P set -t x '@q1' 'he said "hi"' $SEP set -t x '@q2' 'C:\path\x y' | Out-Null
    Check ((Opt x '@q1') -eq 'he said "hi"' -and (Opt x '@q2') -eq 'C:\path\x y') "quotes and backslashes survive the wire line" "@q1='$(Opt x '@q1')' @q2='$(Opt x '@q2')'"

    Write-Head "H: an attached new-session runs the list before the client draws"
    $client = Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -s att $SEP split-window -h $SEP set -g @att yes" -PassThru
    $script:Opened += $client.Id
    $panes = ''
    for ($i = 0; $i -lt 40 -and $panes -notmatch '2'; $i++) { Start-Sleep -Milliseconds 250; $panes = ((P display-message -t att -p '#{window_panes}') -join '').Trim() }
    $clients = (P list-clients) -join ' '
    Check ($panes -eq '2' -and $clients -match 'att' -and -not $client.HasExited) "attached: 2 panes and a client on att" "panes='$panes' clients='$clients' exited=$($client.HasExited)"

    Write-Head "I: kill-server in a list ends the whole namespace"
    $r = Run display-message -t x -p bye $SEP kill-server
    Start-Sleep -Milliseconds 800
    $left = Run ls
    Check ($r.Out -eq 'bye' -and $left.Rc -ne 0) "kill-server after display ended every session" "out='$($r.Out)' ls rc=$($left.Rc) '$($left.Out)'"
} finally {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    foreach ($id in $script:Opened) { try { Stop-Process -Id $id -Force -EA SilentlyContinue } catch {} }
    Get-ChildItem "$psmuxDir\${NS}__*" -EA SilentlyContinue | Remove-Item -Force -EA SilentlyContinue
}

$defaultAfter = ((& $PSMUX ls 2>&1) -join "`n")
Check ($defaultBefore -eq $defaultAfter) "default namespace untouched" "default namespace changed: before '$defaultBefore' after '$defaultAfter'"

Write-Host "`nPASS: $($script:TestsPassed)  FAIL: $($script:TestsFailed)"
exit $script:TestsFailed
