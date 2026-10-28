# #925: paste buffers and the session a target-less buffer command reaches.
#
# -Mode shared: one server holds every session, and its paste buffers are one
# set for the server, as tmux keeps them (paste.c:32 to :36). A buffer loaded
# with no -t, from outside psmux or from a pane, pastes into any session.
# -Mode default: each session is its own server with its own buffers (the
# per session model). The checks pin what that model must still do right: a
# target-less buffer command goes to the session other target-less commands
# use (the pane's own session inside a pane), and a missing buffer is an
# error with tmux's text and exit 1, never a silent success.
#
# Isolation: a unique -L namespace, a private PSMUX_DATA_DIR, an empty config;
# cleanup only with `-L <ns> kill-server`. Pass -Exe or set PSMUX_TEST_EXE.
param([string]$Exe = $(if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }),
      [ValidateSet("shared", "default")][string]$Mode = "shared")

$ErrorActionPreference = "Continue"
$script:Passed = 0
$script:Failed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Passed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Failed++ }

$saved = @{ PSMUX_DATA_DIR = $env:PSMUX_DATA_DIR; PSMUX_SERVER_MODE = $env:PSMUX_SERVER_MODE; PSMUX_CONFIG_FILE = $env:PSMUX_CONFIG_FILE; TMUX = $env:TMUX }
Remove-Item Env:\PSMUX_DATA_DIR, Env:\TMUX -EA SilentlyContinue
$defaultBefore = (& $Exe ls 2>&1 | Out-String).Trim()

$ns = "t925$(Get-Random)"
$data = Join-Path $env:TEMP "psmux_$ns"
New-Item -ItemType Directory $data -Force | Out-Null
$env:PSMUX_DATA_DIR = $data
$empty = Join-Path $data "empty.conf"
"" | Set-Content $empty
$env:PSMUX_CONFIG_FILE = $empty
if ($Mode -eq "shared") { $env:PSMUX_SERVER_MODE = "shared" } else { Remove-Item Env:\PSMUX_SERVER_MODE -EA SilentlyContinue }
function P { & $Exe -L $ns @args 2>&1 }
function Lsb($s) { ((P list-buffers -t "=${s}:") | Out-String).Trim() }
function Wait-Pane($s, $pat) {
    for ($i = 0; $i -lt 40; $i++) {
        $t = (P capture-pane -p -t "=${s}:") | Out-String
        if ($t -match $pat) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

Write-Host "`n=== #925 buffer scope ($Mode) ===" -ForegroundColor Cyan
try {
    foreach ($s in 'a', 'b', 'c') { P new-session -d -s $s -x 120 -y 30 | Out-Null; Start-Sleep -Milliseconds 700 }
    Start-Sleep -Milliseconds 800
    $pids = foreach ($s in 'a', 'b', 'c') { (P display -p -t "=${s}:" '#{pid}') -join '' }
    if ($Mode -eq "shared") {
        if (($pids | Select-Object -Unique).Count -eq 1) { Write-Pass "a, b and c share one server (pid $($pids[0]))" } else { Write-Fail "not one shared server: $($pids -join ',')" }
    } else {
        if (($pids | Select-Object -Unique).Count -eq 3) { Write-Pass "a, b and c are three servers" } else { Write-Fail "expected three servers: $($pids -join ',')" }
    }
    $default = ((P display -p '#{session_name}') -join '').Trim()

    # The issue's repro: load-buffer with no -t, then use it with -t.
    "hola925" | & $Exe -L $ns load-buffer -b g - 2>&1 | Out-Null
    $loadExit = $LASTEXITCODE
    if ($loadExit -eq 0) { Write-Pass "load-buffer -b g - exits 0" } else { Write-Fail "load-buffer exit $loadExit" }

    if ($Mode -eq "shared") {
        foreach ($s in 'a', 'b', 'c') {
            $o = ((P show-buffer -b g -t "=${s}:") | Out-String).Trim()
            if ($o -eq "hola925") { Write-Pass "show-buffer -b g -t =${s}: finds the target-less buffer" } else { Write-Fail "show-buffer -b g -t =${s}: gave '$o' exit $LASTEXITCODE" }
        }
        $o = P paste-buffer -b g -t "=a:"
        $e = $LASTEXITCODE
        if ($e -eq 0 -and (Wait-Pane 'a' 'hola925')) { Write-Pass "paste-buffer -b g -t =a: pastes it into a" } else { Write-Fail "paste-buffer -t =a: exit $e '$o'" }

        # One automatic name sequence for the server.
        P set-buffer -t "=a:" auto_from_a | Out-Null
        P set-buffer -t "=b:" auto_from_b | Out-Null
        $l = Lsb 'c'
        if ($l -match '(?s)buffer1: \d+ bytes: "auto_from_b".*buffer0: \d+ bytes: "auto_from_a"') { Write-Pass "automatic buffers from a and b are buffer0, buffer1 in c" } else { Write-Fail "list-buffers in c: $l" }

        # Deleted in one session, gone in every other.
        P delete-buffer -b g -t "=b:" | Out-Null
        $o = ((P show-buffer -b g -t "=c:") | Out-String).Trim()
        if ($LASTEXITCODE -eq 1 -and $o -match 'no buffer g') { Write-Pass "delete-buffer in b removes g for c" } else { Write-Fail "after delete in b, c: exit $LASTEXITCODE '$o'" }

        # A copy mode yank in a is a buffer in b.
        P send-keys -t "=a:" "echo yank925marker" Enter | Out-Null
        Start-Sleep -Milliseconds 800
        P copy-mode -t "=a:" | Out-Null
        P send-keys -t "=a:" -X search-backward "yank925marker" | Out-Null
        P send-keys -t "=a:" -X select-line | Out-Null
        P send-keys -t "=a:" -X copy-selection-and-cancel | Out-Null
        Start-Sleep -Milliseconds 300
        $o = ((P show-buffer -t "=b:") | Out-String)
        if ($o -match 'yank925marker') { Write-Pass "a copy mode yank in a is the top buffer in b" } else { Write-Fail "show-buffer in b after a yank in a: '$($o.Trim())'" }
    } else {
        $where = @('a', 'b', 'c') | Where-Object { (Lsb $_) -match '^g: ' }
        if ($where -eq $default) { Write-Pass "target-less load-buffer went to the default session ($default), as display-message does" } else { Write-Fail "target-less load-buffer went to '$($where -join ',')', default session is '$default'" }
        $other = @('a', 'b', 'c') | Where-Object { $_ -ne $default } | Select-Object -First 1
        $o = ((P paste-buffer -b g -t "=${other}:") | Out-String).Trim()
        if ($LASTEXITCODE -eq 1 -and $o -match 'no buffer g') { Write-Pass "paste-buffer -b g -t =${other}: is 'no buffer g', exit 1 (not silent)" } else { Write-Fail "paste-buffer -t =${other}: exit $LASTEXITCODE '$o'" }
        $o = ((P show-buffer -b g -t "=${other}:") | Out-String).Trim()
        if ($LASTEXITCODE -eq 1 -and $o -match 'no buffer g') { Write-Pass "show-buffer -b g -t =${other}: is 'no buffer g', exit 1" } else { Write-Fail "show-buffer -t =${other}: exit $LASTEXITCODE '$o'" }
    }

    # From inside a pane of a session that is not the default one: TMUX names
    # the pane's session, and a target-less load-buffer must reach it (default
    # mode) or the server (shared mode).
    $inner = @('a', 'b', 'c') | Where-Object { $_ -ne $default } | Select-Object -First 1
    $out = Join-Path $data "inpane.txt"
    $cmd = "'inpane925' | & '$Exe' -L $ns load-buffer -b ip - ; & '$Exe' -L $ns display-message -p '#{session_name}' | Out-File -Encoding ascii '$out'"
    P send-keys -t "=${inner}:" $cmd Enter | Out-Null
    for ($i = 0; $i -lt 40 -and -not (Test-Path $out); $i++) { Start-Sleep -Milliseconds 250 }
    Start-Sleep -Milliseconds 300
    $seen = ((Get-Content $out -EA SilentlyContinue) -join '').Trim()
    if ($seen -eq $inner) { Write-Pass "inside a pane of $inner, a target-less command names $inner" } else { Write-Fail "inside a pane of ${inner}: '$seen'" }
    if ($Mode -eq "shared") {
        $o = ((P show-buffer -b ip -t "=${default}:") | Out-String).Trim()
        if ($o -eq "inpane925") { Write-Pass "the buffer loaded in a pane of $inner is in $default too" } else { Write-Fail "show-buffer -b ip -t =${default}: '$o'" }
    } else {
        if ((Lsb $inner) -match '(?m)^ip: ') { Write-Pass "the buffer loaded in a pane of $inner went to $inner" } else { Write-Fail "list-buffers in ${inner}: $(Lsb $inner)" }
    }
} finally {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
    foreach ($k in $saved.Keys) { if ($null -ne $saved[$k]) { Set-Item "Env:\$k" $saved[$k] } else { Remove-Item "Env:\$k" -EA SilentlyContinue } }
    Remove-Item Env:\PSMUX_DATA_DIR -EA SilentlyContinue
    $defaultAfter = (& $Exe ls 2>&1 | Out-String).Trim()
    if ($null -ne $saved.PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR = $saved.PSMUX_DATA_DIR }
    if ($defaultAfter -ne $defaultBefore) { Write-Fail "the default namespace changed: before '$defaultBefore' after '$defaultAfter'" }
    Remove-Item $data -Recurse -Force -EA SilentlyContinue
}

Write-Host "`n  Passed: $script:Passed  Failed: $script:Failed"
exit $(if ($script:Failed -eq 0) { 0 } else { 1 })
