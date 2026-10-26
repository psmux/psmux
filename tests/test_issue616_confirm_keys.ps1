# Issue #616 follow up: confirm-before as tmux has it (cmd-confirm-before.c),
# and the Y key of a non Latin layout.
#
# Under a Russian layout the Y key types `н`, and psmux's confirm box only
# knew `y`, so `prefix x` could not be confirmed. Checked here:
#
#  A. an attached client driven by real console key records
#     (WriteConsoleInput, tests/injector.cs): prefix x answered by y, n, Enter,
#     a stray key, `-c н`, `-y` + Enter, and the same from a root binding
#     (which goes through the server's prompt)
#  B. the server's prompt through the CLI and raw TCP: -p shown as given, the
#     default `Confirm '<cmd>'? (y/n)`, confirm_key / confirm_default_yes in
#     dump-state, a single quoted command string runs on yes, `-c ab` refused
#  C. (-RussianLayout only) the Russian Y key confirms and the N key does not.
#     This loads the Russian layout for the length of the run
#     (tests/kl_hold_russian.cs, KLF_NOTELLSHELL, nothing written to the
#     registry) and checks the layout list is back as it was afterwards.
#
# Isolation: a unique -L namespace per case and a private PSMUX_DATA_DIR,
# cleaned up with `-L <ns> kill-server` only.
# Binary: $env:PSMUX_EXE, else the psmux on PATH.
param([switch]$RussianLayout)

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor Yellow; $script:TestsSkipped++ }

$work = Join-Path $env:TEMP ("psmux_616_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $work | Out-Null
$savedDataDir = $env:PSMUX_DATA_DIR
$savedConfig = $env:PSMUX_CONFIG_FILE
$env:PSMUX_DATA_DIR = Join-Path $work "data"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$injector = Join-Path $work "injector.exe"
& $csc /nologo /optimize /out:$injector "$PSScriptRoot\injector.cs" 2>&1 | Out-Null
$injectLog = Join-Path $env:TEMP "psmux_inject.log"

function Set-Config([string]$text) {
    $env:PSMUX_CONFIG_FILE = Join-Path $work ("conf_" + [guid]::NewGuid().ToString('N').Substring(0, 6) + ".conf")
    Set-Content $env:PSMUX_CONFIG_FILE $text -Encoding UTF8
}

function Panes([string]$ns) { (& $PSMUX -L $ns display-message -t t -p '#{window_panes}' 2>&1 | Out-String).Trim() }

# One attached client, three panes, a binding, the keys, then count panes.
function Run-Case([string]$name, [string]$conf, [string]$keys, [int]$want) {
    $ns = "c616_" + [guid]::NewGuid().ToString('N').Substring(0, 8)
    Set-Config $conf
    $proc = Start-Process -FilePath $PSMUX -ArgumentList "-L", $ns, "new-session", "-s", "t" -PassThru
    try {
        $ready = $false
        for ($i = 0; $i -lt 60; $i++) { Start-Sleep -Milliseconds 200; if ((Panes $ns) -eq "1") { $ready = $true; break } }
        if (-not $ready) { Write-Fail "$name : session never came up"; return }
        & $PSMUX -L $ns split-window -t t 2>&1 | Out-Null
        & $PSMUX -L $ns split-window -t t 2>&1 | Out-Null
        Start-Sleep -Milliseconds 1500
        $start = Panes $ns
        & $injector $proc.Id $keys | Out-Null
        $after = $start
        for ($i = 0; $i -lt 10; $i++) { Start-Sleep -Milliseconds 200; $after = Panes $ns; if ($after -ne $start) { break } }
        Start-Sleep -Milliseconds 300
        $after = Panes $ns
        $log = (Get-Content $injectLog -EA SilentlyContinue | Select-String "ok=False")
        if ($log) { Write-Skip "$name : the injector could not write the keys ($log)"; return }
        if ($start -eq "3" -and $after -eq "$want") { Write-Pass "$name : panes $start -> $after" }
        else { Write-Fail "$name : panes $start -> $after, wanted 3 -> $want" }
    } finally {
        & $PSMUX -L $ns kill-server 2>&1 | Out-Null
        Start-Sleep -Milliseconds 500
        try { Stop-Process -Id $proc.Id -Force -EA Stop } catch {}
    }
}

$defaultBefore = $null
try {
    $saved = $env:PSMUX_DATA_DIR; Remove-Item Env:\PSMUX_DATA_DIR
    $defaultBefore = (& $PSMUX ls 2>&1 | Out-String).Trim()
    $env:PSMUX_DATA_DIR = $saved

    $RU_Y = "{MOD:59:043D:0000}"   # the Y key under a Russian layout: VK_Y, 'н'
    $RU_N = "{MOD:4E:0442:0000}"   # the N key under a Russian layout: VK_N, 'т'

    Write-Host "`n=== Part A: attached client, real key records ===" -ForegroundColor Cyan
    if (-not (Test-Path $injector)) {
        Write-Skip "could not compile tests/injector.cs"
    } else {
        Run-Case "prefix x + y kills"                     ""                                "^b{SLEEP:300}x{SLEEP:600}y"             2
        Run-Case "prefix x + n keeps"                     ""                                "^b{SLEEP:300}x{SLEEP:600}n"             3
        Run-Case "prefix x + Enter keeps (tmux: no)"      ""                                "^b{SLEEP:300}x{SLEEP:600}{ENTER}"       3
        Run-Case "prefix x + q closes the box (any key)"  ""                                "^b{SLEEP:300}x{SLEEP:600}q{SLEEP:300}y" 3
        Run-Case "confirm -c н: н kills"                  "bind x confirm -c н kill-pane"   "^b{SLEEP:300}x{SLEEP:600}$RU_Y"         2
        Run-Case "confirm -c н: y keeps"                  "bind x confirm -c н kill-pane"   "^b{SLEEP:300}x{SLEEP:600}y"             3
        Run-Case "confirm -y: Enter kills"                "bind x confirm -y kill-pane"     "^b{SLEEP:300}x{SLEEP:600}{ENTER}"       2
        Run-Case "root F5 confirm -y: Enter kills"        "bind -n F5 confirm -y kill-pane" "{F5}{SLEEP:800}{ENTER}"                 2
        Run-Case "root F5 confirm: y kills"               "bind -n F5 confirm kill-pane"    "{F5}{SLEEP:800}y"                       2
        Run-Case "root F5 confirm: Enter keeps"           "bind -n F5 confirm kill-pane"    "{F5}{SLEEP:800}{ENTER}"                 3
    }

    Write-Host "`n=== Part B: the server's prompt (CLI + TCP) ===" -ForegroundColor Cyan
    $ns = "c616s_" + [guid]::NewGuid().ToString('N').Substring(0, 8)
    Set-Config ""
    & $PSMUX -L $ns new-session -d -s t
    Start-Sleep -Seconds 2
    $portFile = Get-ChildItem $env:PSMUX_DATA_DIR -Filter "${ns}__t.port" | Select-Object -First 1
    $keyFile = Get-ChildItem $env:PSMUX_DATA_DIR -Filter "${ns}__t.key" | Select-Object -First 1
    if (-not $portFile -or -not $keyFile) {
        Write-Fail "port/key file for ${ns}__t not found"
    } else {
        $port = [int](Get-Content $portFile.FullName -Raw).Trim()
        $key = (Get-Content $keyFile.FullName -Raw).Trim()
        function Tcp([string]$line, [int]$timeout = 1500) {
            $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", $port)
            $tcp.ReceiveTimeout = $timeout
            $s = $tcp.GetStream(); $w = [IO.StreamWriter]::new($s); $r = [IO.StreamReader]::new($s)
            $w.Write("AUTH $key`n"); $w.Flush(); $null = $r.ReadLine()
            $w.Write("$line`n"); $w.Flush()
            $out = [System.Collections.Generic.List[string]]::new()
            try { while ($null -ne ($l = $r.ReadLine())) { $out.Add($l); if ($l.Length -gt 100) { break } } } catch {}
            $tcp.Close()
            return $out
        }
        function Dump { (Tcp "dump-state" 3000 | Where-Object { $_.Length -gt 100 } | Select-Object -First 1) | ConvertFrom-Json }

        & $PSMUX -L $ns confirm-before -t t -p "Delete everything?" "set -g @c616 yes" 2>&1 | Out-Null
        Start-Sleep -Milliseconds 600
        $d = Dump
        if ($d.confirm_active -eq $true -and $d.confirm_prompt -eq "Delete everything?") { Write-Pass "-p is shown as given: '$($d.confirm_prompt)'" }
        else { Write-Fail "confirm_active=$($d.confirm_active) prompt='$($d.confirm_prompt)'" }
        if ($d.confirm_key -eq "y" -and $d.confirm_default_yes -eq $false) { Write-Pass "dump-state: confirm_key=y, confirm_default_yes=false" }
        else { Write-Fail "dump-state: confirm_key='$($d.confirm_key)' confirm_default_yes=$($d.confirm_default_yes)" }
        $null = Tcp "confirm-respond y"
        Start-Sleep -Milliseconds 500
        $v = (& $PSMUX -L $ns show-options -gqv "@c616" 2>&1 | Out-String).Trim()
        if ($v -eq "yes") { Write-Pass "yes ran the quoted command string" } else { Write-Fail "yes did not run 'set -g @c616 yes' (@c616='$v')" }

        & $PSMUX -L $ns confirm-before -t t -y -c н kill-window 2>&1 | Out-Null
        Start-Sleep -Milliseconds 600
        $d = Dump
        if ($d.confirm_prompt -eq "Confirm 'kill-window'? (н/n)") { Write-Pass "tmux's default prompt: '$($d.confirm_prompt)'" }
        else { Write-Fail "default prompt '$($d.confirm_prompt)'" }
        if ($d.confirm_key -eq "н" -and $d.confirm_default_yes -eq $true) { Write-Pass "dump-state: confirm_key=н, confirm_default_yes=true" }
        else { Write-Fail "dump-state: confirm_key='$($d.confirm_key)' confirm_default_yes=$($d.confirm_default_yes)" }
        $null = Tcp "confirm-respond n"
        Start-Sleep -Milliseconds 500
        $d = Dump
        if ($d.confirm_active -eq $false -and (Panes $ns) -eq "1") { Write-Pass "no closes the prompt and runs nothing" }
        else { Write-Fail "after no: confirm_active=$($d.confirm_active)" }

        $err = (Tcp "confirm-before -c ab kill-window") -join " "
        Start-Sleep -Milliseconds 400
        $d = Dump
        if ($err -match "invalid confirm key" -and $d.confirm_active -eq $false) { Write-Pass "-c ab refused: '$err'" }
        else { Write-Fail "-c ab: reply '$err', confirm_active=$($d.confirm_active)" }
    }
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null

    Write-Host "`n=== Part C: the Y key of a Russian layout ===" -ForegroundColor Cyan
    if (-not $RussianLayout) {
        Write-Skip "run with -RussianLayout to load the Russian layout for the test"
    } elseif (-not (Test-Path $injector)) {
        Write-Skip "could not compile tests/injector.cs"
    } else {
        $kl = Join-Path $work "kl_hold_russian.exe"
        & $csc /nologo /optimize /out:$kl "$PSScriptRoot\kl_hold_russian.cs" 2>&1 | Out-Null
        $flag = Join-Path $work "kl.done"
        $klOut = Join-Path $work "kl.out"
        $klProc = Start-Process $kl -ArgumentList $flag -PassThru -NoNewWindow -RedirectStandardOutput $klOut
        try {
            Start-Sleep -Seconds 1
            $info = (Get-Content $klOut) -join " "
            if ($info -notmatch "vk=0x59") { Write-Skip "the Russian layout did not load as expected: $info" }
            else {
                Run-Case "Russian Y key (н) kills" "" "^b{SLEEP:300}x{SLEEP:600}$RU_Y" 2
                Run-Case "Russian N key (т) keeps" "" "^b{SLEEP:300}x{SLEEP:600}$RU_N" 3
            }
        } finally {
            New-Item -ItemType File $flag -Force | Out-Null
            $klProc.WaitForExit(15000) | Out-Null
            $lines = Get-Content $klOut
            $beforeList = ($lines | Where-Object { $_ -like "before=*" }) -replace '^before=', ''
            $afterList = ($lines | Where-Object { $_ -like "after=*" }) -replace '^after=', ''
            if ($beforeList -and $beforeList -eq $afterList) { Write-Pass "keyboard layouts back as they were ($afterList)" }
            else { Write-Fail "keyboard layouts before '$beforeList', after '$afterList'" }
        }
    }
} finally {
    $env:PSMUX_DATA_DIR = $savedDataDir
    if ($null -eq $savedDataDir) { Remove-Item Env:\PSMUX_DATA_DIR -EA SilentlyContinue }
    $env:PSMUX_CONFIG_FILE = $savedConfig
    if ($null -eq $savedConfig) { Remove-Item Env:\PSMUX_CONFIG_FILE -EA SilentlyContinue }
    $defaultAfter = (& $PSMUX ls 2>&1 | Out-String).Trim()
    if ($defaultBefore -ne $null -and $defaultAfter -ne $defaultBefore) {
        Write-Fail "the default namespace changed: '$defaultBefore' -> '$defaultAfter'"
    }
    Remove-Item -Recurse -Force $work -EA SilentlyContinue
}

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed:  $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed:  $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
Write-Host "  Skipped: $($script:TestsSkipped)" -ForegroundColor Yellow
exit $script:TestsFailed
