# Issue #1118: Ctrl+\ (C-\) could not be used as a prefix or as a key binding.
#
# Three things were wrong, stacked:
#   1. The server put the formatted prefix into the client state JSON without
#      escaping it, so `set -g prefix C-\` made every frame `"prefix":"C-\"`,
#      which is invalid JSON: the client drew nothing and no prefix worked.
#   2. The key name parser did not take the config forms of C-\ that tmux takes
#      (`"C-\\"`, `C-\\`, `M-\\`): "unknown key".
#   3. (checked here) a real Ctrl+\ keypress into an attached client.
#
# Test 1: bind-key C-\ in every config quoting form tmux accepts, and M-\.
# Test 2: bind-key C-\ from the command line.
# Test 3: set -g prefix / prefix2 C-\ in each config form: show-options,
#         list-keys, and the client state frame (dump-state) parses as JSON.
# Test 4: real Ctrl+\ keypresses (WriteConsoleInput, VK_OEM_5 0xDC with the
#         character 0x1C, the record conhost makes for Ctrl+\) into an attached
#         client: the client draws, Ctrl+\ % splits as the prefix and as
#         prefix2, and a root binding bind -n C-\ fires.
#
# Isolation: own -L namespace per server, own PSMUX_DATA_DIR, cleanup only with
# `-L <ns> kill-server` and by the PID of the clients this script started.
# PSMUX_TEST_BIN picks the binary (default: psmux on PATH).
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue1118_ctrl_backslash.ps1

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } elseif ($env:PSMUX_BIN) { $env:PSMUX_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:TestsFailed++ }
function Check($ok, $m) { if ($ok) { Write-Pass $m } else { Write-Fail $m } }

foreach ($v in 'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'TMUX', 'TMUX_PANE') { Remove-Item "Env:\$v" -EA SilentlyContinue }
$root = Join-Path $env:TEMP "psmux_i1118_$(Get-Random)"
New-Item -ItemType Directory -Force $root | Out-Null
$saved = @{}
foreach ($v in 'PSMUX_DATA_DIR', 'PSMUX_NO_WARM', 'PSMUX_RUN_NOPAUSE') { $saved[$v] = [Environment]::GetEnvironmentVariable($v, 'Process') }
$env:PSMUX_DATA_DIR = $root
$env:PSMUX_NO_WARM = '1'
$env:PSMUX_RUN_NOPAUSE = '1'

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$injector = Join-Path $root 'injector.exe'
$conread = Join-Path $root 'conread.exe'
& $csc /nologo /platform:x64 /out:$injector "$PSScriptRoot\injector.cs" 2>&1 | Out-Null
& $csc /nologo /platform:x64 /out:$conread "$PSScriptRoot\conread.cs" 2>&1 | Out-Null
if (-not (Test-Path $injector) -or -not (Test-Path $conread)) {
    Write-Host "  [FAIL] could not build the C# harnesses with $csc" -ForegroundColor Red
    exit 1
}

$BS = [char]0x5C          # one backslash, so no line below depends on quoting
$CBS = "C-$BS"            # C-\
$script:Opened = @()
$script:Namespaces = @()

function New-Conf($name, [string[]]$lines) {
    $p = Join-Path $root $name
    [IO.File]::WriteAllText($p, (($lines -join "`n") + "`n"))
    $p
}
function Start-Detached($conf) {
    $ns = "r18x1118$(Get-Random)"
    $script:Namespaces += $ns
    $warn = & $PSMUX -L $ns -f $conf new-session -d -s p 2>&1 | Out-String
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 10000) {
        & $PSMUX -L $ns has-session -t p 2>$null
        if ($LASTEXITCODE -eq 0) { break }
        Start-Sleep -Milliseconds 100
    }
    @{ ns = $ns; warn = $warn }
}
function Start-Client($conf) {
    $ns = "r18x1118$(Get-Random)"
    $script:Namespaces += $ns
    $p = Start-Process -FilePath $PSMUX -PassThru -ArgumentList @('-L', $ns, '-f', $conf, 'new-session', '-s', 'p', '-x', '100', '-y', '30')
    $script:Opened += $p
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 15000) {
        $c = & $PSMUX -L $ns list-clients 2>$null
        if ($c) { break }
        Start-Sleep -Milliseconds 150
    }
    # the shell prompt, so a split has something to split
    $sw.Restart()
    while ($sw.ElapsedMilliseconds -lt 15000) {
        $t = (& $PSMUX -L $ns capture-pane -p -t p 2>$null | Out-String).Trim()
        if ($t) { break }
        Start-Sleep -Milliseconds 150
    }
    Start-Sleep -Milliseconds 500
    @{ ns = $ns; proc = $p }
}
function Stop-Ns($ns, $p) {
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
    if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force -EA SilentlyContinue }
}
function Get-Opt($ns, $name) { (& $PSMUX -L $ns show-options -gv $name 2>$null | Out-String).Trim() }
function Get-Panes($ns) { @(& $PSMUX -L $ns list-panes -t p 2>$null).Count }

# One dump-state frame over the server's TCP port, the frame the client parses.
function Get-Frame($ns) {
    $port = (Get-Content (Join-Path $root "${ns}__p.port") -EA SilentlyContinue | Select-Object -First 1)
    $key = (Get-Content (Join-Path $root "${ns}__p.key") -EA SilentlyContinue | Select-Object -First 1)
    if (-not $port) { return $null }
    try {
        $tcp = New-Object Net.Sockets.TcpClient('127.0.0.1', [int]$port.Trim())
        $tcp.ReceiveTimeout = 5000
        $s = $tcp.GetStream()
        $w = New-Object IO.StreamWriter($s); $r = New-Object IO.StreamReader($s)
        $w.WriteLine("AUTH $($key.Trim())"); $w.Flush()
        if ($r.ReadLine() -ne 'OK') { $tcp.Close(); return $null }
        $w.WriteLine('dump-state'); $w.Flush()
        $line = $r.ReadLine()
        $tcp.Close()
        $line
    } catch { $null }
}

# Ctrl+\ as conhost reports it: VK_OEM_5 (0xDC), character 0x1C, left Ctrl.
$CTRL_BS = '{MOD:DC:001C:0008}'

try {
    Write-Host "`n=== Issue #1118: Ctrl+\ as a prefix and as a key binding ===" -ForegroundColor Cyan

    Write-Host "`n[Test 1] bind-key C-\ in the config, every form tmux takes" -ForegroundColor Yellow
    $conf = New-Conf 'keys.conf' @(
        "bind -T ta `"C-$BS$BS`" set -g @a dq",
        "bind -T tb 'C-$BS' set -g @b sq",
        "bind -T tc C-$BS$BS set -g @c bare",
        "bind -n M-$BS$BS set -g @d meta",
        "bind -T t1 `"M-$BS$BS`" set -g @e meta_dq"
    )
    $s = Start-Detached $conf
    Check ($s.warn -notmatch 'unknown key') "no 'unknown key' warning (got: $($s.warn.Trim()))"
    $keys = & $PSMUX -L $s.ns list-keys 2>&1 | Out-String
    foreach ($m in '@a dq', '@b sq', '@c bare') {
        $line = ($keys -split "`n" | Where-Object { $_ -match [regex]::Escape($m) }) -join ''
        Check ($line -match ('-T t[abc] +C-' + [regex]::Escape("$BS") + ' ')) "list-keys shows the $m binding on C-\: '$($line.Trim())'"
    }
    $line = ($keys -split "`n" | Where-Object { $_ -match '@d meta' }) -join ''
    Check ($line -match ('-T root M-' + [regex]::Escape("$BS") + ' ')) "list-keys shows M-\: '$($line.Trim())'"
    $line = ($keys -split "`n" | Where-Object { $_ -match '@e meta_dq' }) -join ''
    Check ($line -match ('-T t1 M-' + [regex]::Escape("$BS") + ' ')) "list-keys shows M-\ from a double quoted form: '$($line.Trim())'"

    Write-Host "`n[Test 2] bind-key C-\ from the command line" -ForegroundColor Yellow
    $o = & $PSMUX -L $s.ns bind-key -n $CBS set -g '@f' cli 2>&1 | Out-String
    Check ($o -notmatch 'unknown key') "bind-key -n 'C-\' is accepted (got: '$($o.Trim())')"
    $keys = & $PSMUX -L $s.ns list-keys -T root 2>&1 | Out-String
    $line = ($keys -split "`n" | Where-Object { $_ -match '@f cli' }) -join ''
    Check ($line -match ('C-' + [regex]::Escape("$BS") + ' ')) "list-keys shows the command line binding: '$($line.Trim())'"
    $o = & $PSMUX -L $s.ns unbind-key -n $CBS 2>&1 | Out-String
    $keys = & $PSMUX -L $s.ns list-keys -T root 2>&1 | Out-String
    Check (($keys -notmatch '@f cli') -and ($o -notmatch 'unknown')) "unbind-key -n 'C-\' removes it"
    # tmux reads no escapes in an argv: these are refused there, and were on
    # master, so they must still say so rather than vanish.
    foreach ($k in "C-$BS$BS", "$BS$BS", "$BS;", "$BS#") {
        $o = & $PSMUX -L $s.ns bind-key -T targv $k display-message x 2>&1 | Out-String
        Check ($o -match 'unknown key') "command line argv '$k' is 'unknown key' as in tmux (got: '$($o.Trim())')"
    }
    Stop-Ns $s.ns $null

    Write-Host "`n[Test 3] set -g prefix / prefix2 C-\" -ForegroundColor Yellow
    $forms = @(
        @('double quoted', "set -g prefix `"C-$BS$BS`""),
        @('single quoted', "set -g prefix 'C-$BS'"),
        @('bare escaped', "set -g prefix C-$BS$BS")
    )
    foreach ($f in $forms) {
        $conf = New-Conf "pfx.conf" @($f[1], 'unbind C-b')
        $s = Start-Detached $conf
        Check ($s.warn -notmatch 'bad key|unknown key') "$($f[0]): no warning (got: '$($s.warn.Trim())')"
        $pv = Get-Opt $s.ns 'prefix'
        Check ($pv -eq $CBS) "$($f[0]): show-options -gv prefix is C-\ (got '$pv')"
        $sp = (& $PSMUX -L $s.ns list-keys -T prefix 2>&1 | Where-Object { $_ -match 'send-prefix' }) -join ''
        Check ($sp -match ('C-' + [regex]::Escape("$BS") + ' +send-prefix')) "$($f[0]): list-keys has C-\ send-prefix: '$($sp.Trim())'"
        $frame = Get-Frame $s.ns
        $parsed = $null
        try { $parsed = $frame | ConvertFrom-Json -EA Stop } catch {}
        Check ($parsed -and $parsed.prefix -eq $CBS) "$($f[0]): the client state frame parses and carries prefix C-\ ($(if ($frame -match '"prefix":.{0,30}') { $Matches[0] } else { 'no frame' }))"
        Stop-Ns $s.ns $null
    }
    $conf = New-Conf "pfx2.conf" @("set -g prefix2 `"C-$BS$BS`"")
    $s = Start-Detached $conf
    $frame = Get-Frame $s.ns
    $parsed = $null
    try { $parsed = $frame | ConvertFrom-Json -EA Stop } catch {}
    Check ($parsed -and $parsed.prefix2 -eq $CBS -and $parsed.prefix -eq 'C-b') "prefix2 C-\: the frame parses with prefix C-b, prefix2 C-\"
    Stop-Ns $s.ns $null

    Write-Host "`n[Test 4] real Ctrl+\ keypresses into an attached client" -ForegroundColor Yellow
    # prefix C-\
    $conf = New-Conf "live_prefix.conf" @("set -g prefix `"C-$BS$BS`"", 'unbind C-b')
    $c = Start-Client $conf
    $screen = & $conread $c.proc.Id 2>&1 | Out-String
    Check ($screen -match '\[p\]') "the client draws its status line with prefix C-\ (saw '[p]': $($screen -match '\[p\]'))"
    & $injector $c.proc.Id "$CTRL_BS{SLEEP:200}%" 2>&1 | Out-Null
    $n = 0; $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 5000) { $n = Get-Panes $c.ns; if ($n -ge 2) { break }; Start-Sleep -Milliseconds 150 }
    Check ($n -eq 2) "Ctrl+\ % with prefix C-\ splits the window (panes: $n)"
    Stop-Ns $c.ns $c.proc

    # prefix2 C-\ next to the default C-b
    $conf = New-Conf "live_prefix2.conf" @("set -g prefix2 `"C-$BS$BS`"")
    $c = Start-Client $conf
    & $injector $c.proc.Id "$CTRL_BS{SLEEP:200}%" 2>&1 | Out-Null
    $n = 0; $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 5000) { $n = Get-Panes $c.ns; if ($n -ge 2) { break }; Start-Sleep -Milliseconds 150 }
    Check ($n -eq 2) "Ctrl+\ % with prefix2 C-\ splits the window (panes: $n)"
    $before = Get-Panes $c.ns
    & $injector $c.proc.Id "^b{SLEEP:200}%" 2>&1 | Out-Null
    $sw.Restart()
    while ($sw.ElapsedMilliseconds -lt 5000) { $n = Get-Panes $c.ns; if ($n -gt $before) { break }; Start-Sleep -Milliseconds 150 }
    Check ($n -eq $before + 1) "C-b % still splits next to prefix2 C-\ (panes: $before -> $n)"
    Stop-Ns $c.ns $c.proc

    # root bindings typed live: the double quoted config form, and M-\
    $conf = New-Conf "live_root.conf" @("bind -n `"C-$BS$BS`" set -g @hit ctrl", "bind -n M-$BS$BS set -g @hit2 meta")
    $c = Start-Client $conf
    & $injector $c.proc.Id $CTRL_BS 2>&1 | Out-Null
    $v = ''; $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 4000) { $v = Get-Opt $c.ns '@hit'; if ($v) { break }; Start-Sleep -Milliseconds 150 }
    Check ($v -eq 'ctrl') "bind -n `"C-\\`" fires on a real Ctrl+\ (@hit='$v')"
    & $injector $c.proc.Id '{MOD:DC:005C:0002}' 2>&1 | Out-Null
    $v = ''; $sw.Restart()
    while ($sw.ElapsedMilliseconds -lt 4000) { $v = Get-Opt $c.ns '@hit2'; if ($v) { break }; Start-Sleep -Milliseconds 150 }
    Check ($v -eq 'meta') "bind -n M-\\ fires on a real Alt+\ (@hit2='$v')"
    Stop-Ns $c.ns $c.proc

    Write-Host "`n[Test 5] common escaped key forms and line ends, typed live" -ForegroundColor Yellow
    # Config forms most tmux configs carry. Each is pressed after the prefix.
    $conf = New-Conf "live_forms.conf" @(
        "bind $BS$BS split-window -h",
        "bind $BS; set -g @semi hit",
        "bind '`"' set -g @dq hit",
        "bind x $BS",
        "  set -g @cont hit",
        "bind y send-keys -l $BS$BS",
        "set -g @after2 yes"
    )
    $c = Start-Client $conf
    Check ((Get-Opt $c.ns '@after2') -eq 'yes') "a line ending in \\ does not swallow the next line (@after2='$(Get-Opt $c.ns '@after2')')"
    $lk = (& $PSMUX -L $c.ns list-keys -T prefix 2>&1 | Out-String)
    foreach ($want in @("$BS +split-window -h", '; +set -g @semi hit', '" +set -g @dq hit', 'x +set -g @cont hit', "y +send-keys -l $BS$BS")) {
        $line = ($lk -split "`n" | Where-Object { $_ -match ('bind-key -T prefix ' + [regex]::Escape(($want -split ' \+')[0]) + ' ') }) -join ''
        Check ($line -match ([regex]::Escape(($want -split ' \+')[0]) + ' +' + [regex]::Escape(($want -split ' \+', 2)[1]))) "list-keys: '$($line.Trim())'"
    }
    $before = Get-Panes $c.ns
    & $injector $c.proc.Id "^b{SLEEP:200}{MOD:DC:005C:0000}" 2>&1 | Out-Null
    $sw = [Diagnostics.Stopwatch]::StartNew(); $n = $before
    while ($sw.ElapsedMilliseconds -lt 5000) { $n = Get-Panes $c.ns; if ($n -gt $before) { break }; Start-Sleep -Milliseconds 150 }
    Check ($n -eq $before + 1) "C-b \ runs bind \\ split-window -h (panes: $before -> $n)"
    foreach ($t in @(@('{MOD:BA:003B:0000}', '@semi', 'bind \;'), @('{MOD:DE:0022:0010}', '@dq', "bind '`"'"), @('x', '@cont', 'bind x \ continued'))) {
        & $injector $c.proc.Id "^b{SLEEP:200}$($t[0])" 2>&1 | Out-Null
        $v = ''; $sw.Restart()
        while ($sw.ElapsedMilliseconds -lt 4000) { $v = Get-Opt $c.ns $t[1]; if ($v) { break }; Start-Sleep -Milliseconds 150 }
        Check ($v -eq 'hit') "C-b then the key fires $($t[2]) ($($t[1])='$v')"
    }
    & $injector $c.proc.Id "^b{SLEEP:200}y" 2>&1 | Out-Null
    $txt = ''; $sw.Restart()
    while ($sw.ElapsedMilliseconds -lt 4000) { $txt = (& $PSMUX -L $c.ns capture-pane -p 2>$null | Out-String); if ($txt -match ([regex]::Escape($BS) + '\s*$')) { break }; Start-Sleep -Milliseconds 150 }
    $last = (($txt -split "`n") | Where-Object { $_.Trim() } | Select-Object -Last 1)
    # Only the line end is checked here: the binding runs on its own, and none
    # of the next line leaks into it. How many backslashes a bare `\\` in a
    # bound command types (tmux: one, psmux: two, mid line too) is a separate,
    # older difference.
    Check ($last -match ([regex]::Escape($BS) + '\s*$') -and $last -notmatch 'set|after2') "C-b y runs send-keys -l \\ alone: '$($last.Trim())'"
    Stop-Ns $c.ns $c.proc
}
finally {
    foreach ($ns in $script:Namespaces) { & $PSMUX -L $ns kill-server 2>&1 | Out-Null }
    foreach ($p in $script:Opened) { if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } }
    foreach ($v in $saved.Keys) { if ($null -eq $saved[$v]) { Remove-Item "Env:\$v" -EA SilentlyContinue } else { Set-Item "Env:\$v" $saved[$v] } }
    Start-Sleep -Milliseconds 300
    Remove-Item -Recurse -Force $root -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
