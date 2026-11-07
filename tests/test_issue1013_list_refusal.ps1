# Issue #1013: a refused split-window inside a command list.
#
# tmux stops a command list at the first command that fails and the client
# exits 1 with the error on stderr. A refused split-window on its own did that
# since #1000, but inside a list (`split-window ... ; select-layout ...`, the
# usual way tools chain it) psmux exited 0, printed the error on STDOUT and
# ran the rest of the list. The refusal is now the command's error reply.
#
# Checks, in a 20x10 session where a -h split is refused:
#   [1] -P in a list, no -P in a list, a refusal followed by display-message:
#       exit 1, error on stderr, nothing on stdout, the rest did not run.
#   [2] the lone split keeps its #1000 behaviour and wording.
#   [3] a refusal after a command that ran: that one's output stays, the rest stops.
#   [4] other refusals in a list (bad target, unknown command) still stop it.
#   [5] lists that succeed are unchanged: output, exit 0, the new pane id.
#
# Run: pwsh -NoProfile -File tests\test_issue1013_list_refusal.ps1
# PSMUX_TEST_BIN overrides the binary under test.

$ErrorActionPreference = 'Continue'
$env:SHELL = $null
$ExePath = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -ErrorAction Stop).Source }
$DataDir = Join-Path ([IO.Path]::GetTempPath()) "psmux-i1013-$PID"
New-Item -ItemType Directory -Force $DataDir | Out-Null
$env:PSMUX_DATA_DIR = $DataDir
$script:Passed = 0
$script:Failed = 0
$script:NS = "i1013a$PID"
$Conf = Join-Path $DataDir 'empty.conf'
Set-Content -Path $Conf -Value ''

function Check {
    param([bool]$Condition, [string]$Name, [string]$Detail = '')
    if ($Condition) { $script:Passed++; Write-Output "  [PASS] $Name" } else { $script:Failed++; Write-Output "  [FAIL] $Name $Detail" }
}
function P { & $ExePath -L $script:NS @args }
# Runs psmux with the arguments, returning exit code, stdout and stderr apart.
function Run {
    $errFile = Join-Path $DataDir 'stderr.txt'
    $out = & $ExePath -L $script:NS @args 2> $errFile
    [pscustomobject]@{ Rc = $LASTEXITCODE; Out = (($out | Out-String).Trim()); Err = ((Get-Content $errFile -Raw) + '').Trim() }
}
function Opt([string]$Name) { ((P show-options -gqv $Name) | Out-String).Trim() }
function Panes { ((P list-panes -t t -F '#{pane_id} #{pane_width}x#{pane_height}') | Out-String).Trim() }
function Describe($r) { "(rc=$($r.Rc) out=[$($r.Out)] err=[$($r.Err)])" }
$TooSmall = 'split-window: pane too small to split horizontally'

try {
    & $ExePath -f $Conf -L $script:NS new-session -d -s t -x 20 -y 10 | Out-Null
    Start-Sleep -Milliseconds 1500
    $panes0 = Panes
    Check ($panes0 -eq '%1 20x10') "the session has one 20x10 pane" "([$panes0])"

    Write-Output "[1] refused split in a list"
    $r = Run split-window -h -d -P -F '#{pane_id}' -t '%1' ';' set-option -g '@ran' yes
    Check ($r.Rc -eq 1) "-P in a list exits 1" (Describe $r)
    Check ($r.Out -eq '') "-P in a list prints nothing on stdout" (Describe $r)
    Check ($r.Err -match [regex]::Escape($TooSmall)) "-P in a list puts the error on stderr" (Describe $r)
    Check ((Opt '@ran') -eq '') "-P in a list: the rest of the list did not run" "(@ran=[$(Opt '@ran')])"

    $r = Run split-window -h -d -t '%1' ';' set-option -g '@ran2' yes
    Check ($r.Rc -eq 1 -and $r.Out -eq '' -and $r.Err -match [regex]::Escape($TooSmall)) "no -P in a list: exit 1, error on stderr" (Describe $r)
    Check ((Opt '@ran2') -eq '') "no -P in a list: the rest of the list did not run" "(@ran2=[$(Opt '@ran2')])"

    $r = Run split-window -h -d -t '%1' ';' display-message -p second-ran
    Check ($r.Rc -eq 1 -and $r.Out -notmatch 'second-ran') "a refusal then display-message: the second never prints" (Describe $r)

    $r = Run split-window -h -d -P -F '#{pane_id}' ';' select-layout even-horizontal
    Check ($r.Rc -eq 1 -and $r.Out -eq '' -and $r.Err -match [regex]::Escape($TooSmall)) "the reporter's split then select-layout, untargeted" (Describe $r)
    Check ((Panes) -eq $panes0) "no pane was created by any refused split" "([$(Panes)])"

    Write-Output "[2] the lone split keeps #1000's behaviour"
    $r = Run split-window -h -d -P -F '#{pane_id}' -t '%1'
    Check ($r.Rc -eq 1 -and $r.Out -eq '' -and $r.Err -like "psmux: $TooSmall*") "-P alone: exit 1, 'psmux: split-window: ...' on stderr" (Describe $r)
    $r = Run split-window -h -d -t '%1'
    Check ($r.Rc -eq 1 -and $r.Out -eq '' -and $r.Err -like "psmux: $TooSmall*") "no -P alone: exit 1, 'psmux: split-window: ...' on stderr" (Describe $r)

    Write-Output "[3] a refusal after a command that ran"
    $r = Run display-message -p first ';' split-window -h -d ';' set-option -g '@ran3' yes
    Check ($r.Rc -eq 1 -and $r.Out -eq 'first' -and $r.Err -like "psmux: $TooSmall*") "the first command's output stays, the error goes to stderr" (Describe $r)
    Check ((Opt '@ran3') -eq '') "the command after the refusal did not run" "(@ran3=[$(Opt '@ran3')])"

    Write-Output "[4] other failures in a list still stop it"
    $r = Run display-message -p first ';' select-pane -t '%99' ';' set-option -g '@ran4' yes
    Check ($r.Rc -eq 1 -and $r.Err -match "can't find pane: %99" -and (Opt '@ran4') -eq '') "a bad target in the middle stops the list" (Describe $r)
    $r = Run display-message -p first ';' nosuchcmd ';' set-option -g '@ran5' yes
    Check ($r.Rc -eq 1 -and $r.Err -match 'unknown command: nosuchcmd' -and (Opt '@ran5') -eq '') "an unknown command in the middle stops the list" (Describe $r)

    Write-Output "[5] lists that succeed are unchanged"
    $r = Run display-message -p one ';' display-message -p two
    Check ($r.Rc -eq 0 -and $r.Out -eq "one`r`ntwo" -or ($r.Rc -eq 0 -and ($r.Out -replace "`r", '') -eq "one`ntwo")) "a list of two display-messages prints both, exit 0" (Describe $r)
    P resize-window -t t -x 80 -y 20 | Out-Null
    Start-Sleep -Milliseconds 300
    $r = Run split-window -h -d -P -F '#{pane_id}' -t '%1' ';' set-option -g '@ok' yes
    Check ($r.Rc -eq 0 -and $r.Out -match '^%\d+$' -and $r.Err -eq '') "a split that fits prints its pane id, exit 0" (Describe $r)
    Check ((Opt '@ok') -eq 'yes') "the rest of a successful list runs" "(@ok=[$(Opt '@ok')])"
    Check ((Panes) -match [regex]::Escape($r.Out)) "the printed pane exists" "([$(Panes)])"
    $r = Run split-window -v -d ';' display-message -p after
    Check ($r.Rc -eq 0 -and $r.Out -eq 'after' -and $r.Err -eq '') "a split without -P then display-message prints only 'after'" (Describe $r)
}
finally {
    & $ExePath -L $script:NS kill-server 2>$null | Out-Null
    Remove-Item -Recurse -Force $DataDir -ErrorAction SilentlyContinue
}

Write-Output ""
Write-Output "Passed: $($script:Passed)  Failed: $($script:Failed)"
if ($script:Failed -gt 0) { exit 1 } else { exit 0 }
