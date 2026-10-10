# Issue #775: show-environment ignored NAME and -g, a missing variable was
# never `unknown variable: NAME` (exit 1), `set-environment -r NAME` read back
# as `NAME=` instead of `-NAME`, and the server's start environment was never
# listed (only PSMUX*/TMUX* names were).
#
# tmux semantics (cmd-show-environment.c, cmd-set-environment.c, environ.c):
#   * the session environment (no -g) and the global one (-g, the server start
#     environment plus set-environment -g) are separate;
#   * `show-environment NAME` prints NAME=value, `-NAME` for a removal, or
#     `unknown variable: NAME` on stderr at exit 1;
#   * -s prints shell syntax, -h shows only hidden entries;
#   * a new pane gets global <- session, without removed or hidden names.
#
# Layers: CLI, raw TCP, config file, command prompt in an attached client
# (keys via tests\injector.cs, screen via tests\conread.cs), what a new pane
# actually inherits, and a warm claimed server's -g view (#659).
#
# Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$script:TestsPassed = 0; $script:TestsFailed = 0; $script:Skipped = 0
$script:Opened = @()

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow; $script:Skipped++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX_TARGET_SESSION') {
    Remove-Item "Env:$v" -EA SilentlyContinue
}

$NS  = "a775-" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_775_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null
$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"

# Run psmux and capture stdout, stderr and the exit code separately.
function Run {
    $out = Join-Path $TMP "o.txt"; $err = Join-Path $TMP "e.txt"
    $argList = @("-L", $NS) + $args
    $p = Start-Process -FilePath $PSMUX -ArgumentList ($argList | ForEach-Object {
            if ($_ -match '[\s"]' -or $_ -eq '') { '"' + ($_ -replace '"', '\"') + '"' } else { $_ } }) `
        -NoNewWindow -Wait -PassThru -RedirectStandardOutput $out -RedirectStandardError $err
    [pscustomobject]@{
        Out = ([string](Get-Content $out -Raw -EA SilentlyContinue)) -replace "`r", ''
        Err = (([string](Get-Content $err -Raw -EA SilentlyContinue)) -replace "`r", '').Trim()
        Rc  = $p.ExitCode
    }
}

function Kill-Rig {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    foreach ($id in $script:Opened) { try { Stop-Process -Id $id -Force -EA SilentlyContinue } catch {} }
    $script:Opened = @()
    Get-ChildItem "$psmuxDir\${NS}__*" -EA SilentlyContinue | Remove-Item -Force -EA SilentlyContinue
}

function Wait-Session($s) {
    for ($i = 0; $i -lt 60; $i++) {
        & $PSMUX -L $NS has-session -t $s 2>$null
        if ($LASTEXITCODE -eq 0) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

function Tcp($sess, $line) {
    $port = (Get-Content "$psmuxDir\${NS}__$sess.port" -Raw).Trim()
    $key  = (Get-Content "$psmuxDir\${NS}__$sess.key" -Raw).Trim()
    $c = [System.Net.Sockets.TcpClient]::new('127.0.0.1', [int]$port)
    $s = $c.GetStream(); $s.ReadTimeout = 5000
    $w = [System.IO.StreamWriter]::new($s); $w.AutoFlush = $true
    $r = [System.IO.StreamReader]::new($s)
    $w.WriteLine("AUTH $key")
    $null = $r.ReadLine()
    $w.WriteLine($line)
    $c.Client.Shutdown([System.Net.Sockets.SocketShutdown]::Send)
    try { $text = $r.ReadToEnd() } catch { $text = "" }
    $c.Close()
    return ($text -replace "`r", '')
}

$CONF = Join-Path $TMP "a775.conf"
@(
    "set -g default-shell $POWERSHELL",
    "set -g display-time 6000",
    "set-environment -g E775_CFG_G cfg-global",
    "set-environment E775_CFG_S cfg-session",
    "set-environment -r E775_CFG_R",
    "%hidden E775_HID=hidden-val",
    "set -g @e775_from_hidden `$E775_HID"
) | Set-Content -Path $CONF -Encoding ASCII

Kill-Rig
$env:PSMUX_NO_WARM = '1'
$env:E775_INH = 'inherited-val'
$env:E775_GONE = 'start-val'
& $PSMUX -L $NS -f $CONF new-session -d -s s1 2>&1 | Out-Null
Remove-Item Env:E775_INH, Env:E775_GONE
if (-not (Wait-Session s1)) { Write-Fail "session s1 never came up"; Kill-Rig; exit 1 }

# ── 1. CLI: the reporter's steps ──
Write-Head "1. CLI named queries (reporter's steps)"
Run set-environment -t s1 E775_SES session-val | Out-Null
Run set-environment -g E775_GLB global-val | Out-Null
Run set-environment -t s1 -r E775_RM | Out-Null

$r = Run show-environment -t s1 E775_SES
Check ($r.Out -eq "E775_SES=session-val`n" -and $r.Rc -eq 0) "NAME prints exactly that entry" "got [$($r.Out)] rc=$($r.Rc)"
$r = Run show-environment -t s1 E775_NOPE
Check ($r.Err -eq "unknown variable: E775_NOPE" -and $r.Rc -eq 1 -and $r.Out -eq '') "missing NAME is 'unknown variable' rc 1" "got out=[$($r.Out)] err=[$($r.Err)] rc=$($r.Rc)"
$r = Run show-environment -t s1 E775_RM
Check ($r.Out -eq "-E775_RM`n" -and $r.Rc -eq 0) "-r reads back as -NAME" "got [$($r.Out)] rc=$($r.Rc)"
$r = Run show-environment -g E775_INH
Check ($r.Out -eq "E775_INH=inherited-val`n" -and $r.Rc -eq 0) "-g shows the server start environment" "got [$($r.Out)] rc=$($r.Rc)"
$r = Run show-environment -g -t s1 E775_SES
Check ($r.Err -eq "unknown variable: E775_SES" -and $r.Rc -eq 1) "a session only var is not global" "got err=[$($r.Err)] rc=$($r.Rc)"
$r = Run show-environment -t s1 E775_GLB
Check ($r.Rc -eq 1) "a -g var is not in the session environment" "got [$($r.Out)] rc=$($r.Rc)"
$r = Run show-environment -g E775_GLB
Check ($r.Out -eq "E775_GLB=global-val`n") "-g var reads back with -g" "got [$($r.Out)]"
$r = Run show-environment -g
$lines = @($r.Out -split "`n" | Where-Object { $_ })
Check (($lines | Where-Object { $_ -like 'Path=*' -or $_ -like 'PATH=*' }).Count -eq 1 -and ($lines -contains 'E775_INH=inherited-val')) "-g listing has PATH and the inherited var ($($lines.Count) lines)" "listing: $($lines -join ' | ')"
$sorted = @($lines | ForEach-Object { ($_ -replace '^-', '') -replace '=.*$', '' })
$ok = $true; for ($i = 1; $i -lt $sorted.Count; $i++) { if ([string]::CompareOrdinal($sorted[$i-1], $sorted[$i]) -gt 0) { $ok = $false } }
Check $ok "-g listing is sorted like tmux" "unsorted: $($sorted -join ',')"
$r = Run show-environment -t s1
Check (($r.Out -match "(?m)^E775_SES=session-val$") -and ($r.Out -match "(?m)^-E775_RM$") -and ($r.Out -notmatch "(?mi)^PATH=")) "session listing: session vars and -NAME, no global env" "got [$($r.Out)]"

Write-Head "1b. CLI flags: -s, -h, -u, -F and tmux's refusals"
Run set-environment -t s1 E775_SH 'a$b"c' | Out-Null
$r = Run show-environment -s -t s1 E775_SH
Check ($r.Out -eq "E775_SH=`"a\`$b\`"c`"; export E775_SH;`n") "-s escapes and exports" "got [$($r.Out)]"
$r = Run show-environment -s -t s1 E775_RM
Check ($r.Out -eq "unset E775_RM;`n") "-s prints a removal as unset" "got [$($r.Out)]"
Run set-environment -h -t s1 E775_H secret | Out-Null
$r = Run show-environment -t s1 E775_H
Check ($r.Out -eq '' -and $r.Rc -eq 0) "a hidden var prints nothing without -h" "got [$($r.Out)] rc=$($r.Rc)"
$r = Run show-environment -h -t s1 E775_H
Check ($r.Out -eq "E775_H=secret`n") "-h shows the hidden var" "got [$($r.Out)]"
Run set-environment -u -t s1 E775_SH | Out-Null
$r = Run show-environment -t s1 E775_SH
Check ($r.Rc -eq 1) "-u removes the entry entirely" "got [$($r.Out)] rc=$($r.Rc)"
Run set-environment -F -t s1 E775_FMT '#{session_name}' | Out-Null
$r = Run show-environment -t s1 E775_FMT
Check ($r.Out -eq "E775_FMT=s1`n") "-F expands the value" "got [$($r.Out)]"
$r = Run set-environment -t s1 E775_NOVAL
Check ($r.Err -eq "no value specified" -and $r.Rc -eq 1) "NAME without a value is refused" "got err=[$($r.Err)] rc=$($r.Rc)"
$r = Run set-environment -t s1 -r E775_X val
Check ($r.Err -eq "can't specify a value with -r" -and $r.Rc -eq 1) "-r with a value is refused" "got err=[$($r.Err)] rc=$($r.Rc)"
Run set-environment -t s1 E775_SPACE 'two words' | Out-Null
$r = Run show-environment -t s1 E775_SPACE
Check ($r.Out -eq "E775_SPACE=two words`n") "a value with spaces survives the CLI" "got [$($r.Out)]"

# ── 2. TCP ──
Write-Head "2. raw TCP"
$t = Tcp s1 "show-environment E775_SES"
Check ($t -eq "E775_SES=session-val`n") "TCP named query" "got [$t]"
$t = Tcp s1 "show-environment E775_NOPE"
Check ($t.Trim() -eq "ERROR: unknown variable: E775_NOPE") "TCP miss answers the error" "got [$t]"
$null = Tcp s1 "set-environment -r E775_TCPRM"
$t = Tcp s1 "show-environment E775_TCPRM"
Check ($t -eq "-E775_TCPRM`n") "TCP -r is a removal marker" "got [$t]"
$t = Tcp s1 "show-environment -g E775_INH"
Check ($t -eq "E775_INH=inherited-val`n") "TCP -g sees the start environment" "got [$t]"

# ── 3. config file ──
Write-Head "3. config file"
$r = Run show-environment -g E775_CFG_G
Check ($r.Out -eq "E775_CFG_G=cfg-global`n") "config -g var is global" "got [$($r.Out)] err=[$($r.Err)]"
$r = Run show-environment -t s1 E775_CFG_S
Check ($r.Out -eq "E775_CFG_S=cfg-session`n") "config session var" "got [$($r.Out)]"
$r = Run show-environment -t s1 E775_CFG_R
Check ($r.Out -eq "-E775_CFG_R`n") "config -r is a removal marker" "got [$($r.Out)]"
$r = Run show-environment -gh E775_HID
Check ($r.Out -eq "E775_HID=hidden-val`n") "%hidden is a hidden global entry" "got [$($r.Out)]"
$r = Run show-options -gv '@e775_from_hidden'
Check ($r.Out.Trim() -eq "hidden-val") "`$NAME still expands from %hidden" "got [$($r.Out)]"

# ── 4. what a new pane inherits ──
Write-Head "4. new pane environment (environ_for_session)"
Run set-environment -t s1 -r E775_GONE | Out-Null
$dump = Join-Path $TMP "dump.ps1"
$paneOut = Join-Path $TMP "pane_env.txt"
@'
param($out)
$names = 'E775_SES','E775_GLB','E775_INH','E775_GONE','E775_RM','E775_H','E775_HID','E775_CFG_G','E775_CFG_S'
$names | ForEach-Object { "$_=" + [Environment]::GetEnvironmentVariable($_) } | Set-Content -Path $out -Encoding ASCII
'@ | Set-Content -Path $dump -Encoding ASCII
Run new-window -d -t s1 "$POWERSHELL -NoProfile -ExecutionPolicy Bypass -File $dump $paneOut" | Out-Null
for ($i = 0; $i -lt 60 -and -not (Test-Path $paneOut); $i++) { Start-Sleep -Milliseconds 250 }
Start-Sleep -Milliseconds 300
$pe = @{}
if (Test-Path $paneOut) { Get-Content $paneOut | ForEach-Object { $k, $v = $_ -split '=', 2; $pe[$k] = $v } }
Write-Info ("pane saw: " + (($pe.GetEnumerator() | Sort-Object Name | ForEach-Object { "$($_.Name)=$($_.Value)" }) -join ' '))
Check ($pe['E775_SES'] -eq 'session-val') "session var reaches the pane" "got [$($pe['E775_SES'])]"
Check ($pe['E775_GLB'] -eq 'global-val') "-g var reaches the pane" "got [$($pe['E775_GLB'])]"
Check ($pe['E775_INH'] -eq 'inherited-val') "start env var reaches the pane" "got [$($pe['E775_INH'])]"
Check ($pe['E775_GONE'] -eq '') "-r removed start env var does NOT reach the pane" "got [$($pe['E775_GONE'])]"
Check ($pe['E775_H'] -eq '' -and $pe['E775_HID'] -eq '') "hidden vars do not reach the pane" "got H=[$($pe['E775_H'])] HID=[$($pe['E775_HID'])]"
Check ($pe['E775_CFG_G'] -eq 'cfg-global' -and $pe['E775_CFG_S'] -eq 'cfg-session') "config vars reach the pane" "got [$($pe['E775_CFG_G'])] [$($pe['E775_CFG_S'])]"
$r = Run show-environment -g E775_GONE
Check ($r.Out -eq "E775_GONE=start-val`n") "the global value under a session -r is intact" "got [$($r.Out)]"

Kill-Rig

# ── 5. command prompt in an attached client ──
Write-Head "5. command prompt (attached TUI)"
$csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
}
$tuiOk = $true
foreach ($tool in "conread", "injector") {
    $exe = Join-Path $TMP "$tool.exe"
    if ($csc -and (Test-Path $csc)) { & $csc /nologo /optimize /out:$exe (Join-Path $PSScriptRoot "$tool.cs") 2>&1 | Out-Null }
    if (-not (Test-Path $exe)) { $tuiOk = $false }
}
if (-not $tuiOk) {
    Write-Skip "csc.exe unavailable, TUI layer skipped"
} else {
    $RD = Join-Path $TMP "conread.exe"; $INJ = Join-Path $TMP "injector.exe"
    $p = Start-Process -FilePath $PSMUX -ArgumentList "-f",$CONF,"-L",$NS,"new-session","-s","tui","-x","120","-y","30" -PassThru
    $script:Opened += $p.Id
    if (-not (Wait-Session tui)) { Write-Fail "attached session never came up" } else {
        Start-Sleep -Milliseconds 2500
        function Inj($keys) { & $INJ $p.Id $keys | Out-Null; Start-Sleep -Milliseconds 500 }
        function Screen {
            $o = Join-Path $TMP "screen.txt"
            Start-Process -FilePath $RD -ArgumentList "$($p.Id)" -Wait -WindowStyle Hidden -RedirectStandardOutput $o | Out-Null
            if (Test-Path $o) { return ((Get-Content $o) -join "`n") } else { return "" }
        }
        Run set-environment -t tui E775_TUI_S tui-val | Out-Null
        Inj "^b{SLEEP:400}:show-environment E775_TUI_S{ENTER}"
        Start-Sleep -Milliseconds 1200
        $sc = Screen
        Check ($sc -match 'E775_TUI_S=tui-val') "prompt show-environment NAME shows the entry" "screen has no E775_TUI_S=tui-val"
        Check ($sc -notmatch 'E775_CFG_S=') "prompt NAME query is filtered" "the popup still lists other variables"
        Inj "q"; Inj "{ESC}"
        Start-Sleep -Milliseconds 500
        Inj "^b{SLEEP:400}:show-environment E775_TUI_NOPE{ENTER}"
        Start-Sleep -Milliseconds 700
        $sc = Screen
        Check ($sc -match 'unknown variable: E775_TUI_NOPE') "prompt miss shows 'unknown variable' in the status line" "screen: $(($sc -split "`n" | Select-Object -Last 2) -join ' / ')"
        Start-Sleep -Milliseconds 1500
        Inj "^b{SLEEP:400}:set-environment -r E775_TUI_RM{ENTER}"
        Start-Sleep -Milliseconds 700
        $r = Run show-environment -t tui E775_TUI_RM
        Check ($r.Out -eq "-E775_TUI_RM`n") "prompt set-environment -r is a removal marker" "got [$($r.Out)] err=[$($r.Err)]"
    }
    Kill-Rig
}

# ── 6. warm claimed server: -g is the claiming client's environment ──
Write-Head "6. warm claim (#659): -g reflects the claiming client"
Remove-Item Env:PSMUX_NO_WARM -EA SilentlyContinue
& $PSMUX -L $NS new-session -d -s w1 2>&1 | Out-Null
if (Wait-Session w1) {
    Start-Sleep -Milliseconds 3000
    $env:E775_CLAIM = 'claimer-val'
    & $PSMUX -L $NS new-session -d -s w2 2>&1 | Out-Null
    Remove-Item Env:E775_CLAIM
    if (Wait-Session w2) {
        $r = Run show-environment -t w2 -g E775_CLAIM
        Check ($r.Out -eq "E775_CLAIM=claimer-val`n") "-g on a claimed server shows the claiming client's variable" "got [$($r.Out)] err=[$($r.Err)]"
        $r = Run show-environment -t w1 -g E775_CLAIM
        Check ($r.Rc -eq 1) "the first server never had it" "got [$($r.Out)] rc=$($r.Rc)"
    } else { Write-Fail "w2 never came up" }
} else { Write-Fail "w1 never came up" }
Kill-Rig

# ── 7. update-environment: new-session seed, attach from another shell ──
# tmux environ_update: the client's value for each matching pattern, the
# pattern recorded as -NAME when nothing matches (new-session, attach).
Write-Head "7. update-environment from the creating and the attaching client"
$env:PSMUX_NO_WARM = '1'
$UECONF = Join-Path $TMP "ue.conf"
@("set -g default-shell $POWERSHELL", 'set -g update-environment "E775_UE E775_UEMISS E775_UEG*"') | Set-Content $UECONF -Encoding ASCII
$env:E775_UE = 'one'
& $PSMUX -L $NS -f $UECONF new-session -d -s ue 2>&1 | Out-Null
Remove-Item Env:E775_UE
if (-not (Wait-Session ue)) { Write-Fail "session ue never came up" } else {
    $r = Run show-environment -t ue E775_UE
    Check ($r.Out -eq "E775_UE=one`n") "new-session seeds the creating client's value" "got [$($r.Out)] err=[$($r.Err)]"
    $r = Run show-environment -t ue E775_UEMISS
    Check ($r.Out -eq "-E775_UEMISS`n") "a name the client lacks is recorded as -NAME" "got [$($r.Out)] err=[$($r.Err)]"
    $r = Run show-environment -t ue 'E775_UEG*'
    Check ($r.Out -eq "-E775_UEG*`n") "an unmatched glob is recorded as -PATTERN like tmux" "got [$($r.Out)] err=[$($r.Err)]"

    $env:E775_UE = 'two'; $env:E775_UEMISS = 'now-here'; $env:E775_UEGLOB = 'globbed'
    $ap = Start-Process -FilePath $PSMUX -ArgumentList "-L",$NS,"attach","-t","ue" -PassThru
    Remove-Item Env:E775_UE, Env:E775_UEMISS, Env:E775_UEGLOB
    $script:Opened += $ap.Id
    $got = $null
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 250
        $got = (Run show-environment -t ue E775_UE).Out
        if ($got -eq "E775_UE=two`n") { break }
    }
    Check ($got -eq "E775_UE=two`n") "attach takes the ATTACHING client's value" "got [$got]"
    $r = Run show-environment -t ue E775_UEMISS
    Check ($r.Out -eq "E775_UEMISS=now-here`n") "attach fills a name the creator lacked" "got [$($r.Out)]"
    $r = Run show-environment -t ue E775_UEGLOB
    Check ($r.Out -eq "E775_UEGLOB=globbed`n") "attach copies glob matches" "got [$($r.Out)]"
    $ueOut = Join-Path $TMP "ue_pane.txt"
    $ueDump = Join-Path $TMP "ue_dump.ps1"
    'param($o) "E775_UE=" + $env:E775_UE + "|E775_UEMISS=" + $env:E775_UEMISS + "|E775_UEGLOB=" + $env:E775_UEGLOB | Set-Content $o' | Set-Content $ueDump -Encoding ASCII
    Run new-window -d -t ue "$POWERSHELL -NoProfile -ExecutionPolicy Bypass -File $ueDump $ueOut" | Out-Null
    for ($i = 0; $i -lt 60 -and -not (Test-Path $ueOut); $i++) { Start-Sleep -Milliseconds 250 }
    Start-Sleep -Milliseconds 300
    $ueSaw = (Get-Content $ueOut -EA SilentlyContinue) -join ''
    Check ($ueSaw -eq "E775_UE=two|E775_UEMISS=now-here|E775_UEGLOB=globbed") "a new pane after the attach gets the attaching client's values" "pane saw [$ueSaw]"
}
Kill-Rig

# ── 8. display-popup gets the same environment as a new pane ──
Write-Head "8. display-popup environment"
$pp = Start-Process -FilePath $PSMUX -ArgumentList "-f",$CONF,"-L",$NS,"new-session","-s","pp","-x","120","-y","30" -PassThru
$script:Opened += $pp.Id
if (-not (Wait-Session pp)) { Write-Fail "session pp never came up" } else {
    Start-Sleep -Milliseconds 2500
    Run set-environment -t pp -r TEMP | Out-Null
    Run set-environment -t pp -h E775_PH hidden | Out-Null
    Run set-environment -t pp E775_PS sesval | Out-Null
    $ppDump = Join-Path $TMP "pp_dump.ps1"
    'param($o) "TEMP=" + $env:TEMP + "|E775_PH=" + $env:E775_PH + "|E775_PS=" + $env:E775_PS | Set-Content $o' | Set-Content $ppDump -Encoding ASCII
    $ppPane = Join-Path $TMP "pp_pane.txt"; $ppPop = Join-Path $TMP "pp_popup.txt"
    Run new-window -d -t pp "$POWERSHELL -NoProfile -ExecutionPolicy Bypass -File $ppDump $ppPane" | Out-Null
    Run display-popup -t pp -E "$POWERSHELL -NoProfile -ExecutionPolicy Bypass -File $ppDump $ppPop" | Out-Null
    for ($i = 0; $i -lt 60 -and -not ((Test-Path $ppPane) -and (Test-Path $ppPop)); $i++) { Start-Sleep -Milliseconds 250 }
    Start-Sleep -Milliseconds 300
    $sawPane = (Get-Content $ppPane -EA SilentlyContinue) -join ''
    $sawPop = (Get-Content $ppPop -EA SilentlyContinue) -join ''
    Write-Info "pane [$sawPane] popup [$sawPop]"
    Check ($sawPane -eq "TEMP=|E775_PH=|E775_PS=sesval") "pane: -r and hidden dropped, session var set" "pane saw [$sawPane]"
    Check ($sawPop -eq $sawPane) "popup gets exactly the pane's environment" "popup saw [$sawPop]"
}
Kill-Rig

Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
Write-Host "`nResults: $($script:TestsPassed) passed, $($script:TestsFailed) failed, $($script:Skipped) skipped"
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
