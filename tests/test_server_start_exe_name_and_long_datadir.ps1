# A copied or renamed psmux binary must reach the server it starts, a psmux.exe
# CLI sharing the data dir must not reap that live server, and a long data dir
# path or a reused data dir must still start a server.
#
# Before the fix: `pr856_master.exe new-session -d -s x` exited 0, the server
# ran and listened, but every later command said "no server running": the pid
# anchor liveness check trusted only the image names psmux, tmux and pmux, so
# it called the server's own PID "recycled by another image". A psmux.exe CLI
# in the same data dir deleted that live server's registry files and then
# failed with "failed to create session".
#
# Safety: every server runs under a unique -L namespace with a private
# PSMUX_DATA_DIR. Cleanup is `-L <ns> kill-server`, then by recorded PID only.

$ErrorActionPreference = 'Continue'
$exe = $env:PSMUX_TEST_EXE
if (-not $exe) { $exe = (Get-Command psmux -ErrorAction Stop).Source }
$exe = (Resolve-Path $exe).Path

$root = $env:PSMUX_TEST_ROOT
if (-not $root) { $root = Join-Path $env:TEMP "psmux_exe_name_$PID" }
New-Item -ItemType Directory -Force $root | Out-Null

$pass = 0; $fail = 0
function Check($name, $ok, $detail = '') {
    if ($ok) { $script:pass++; Write-Host "PASS  $name" -ForegroundColor Green }
    else { $script:fail++; Write-Host "FAIL  $name  $detail" -ForegroundColor Red }
}

$tag = "sfx$PID"
$started = @()   # (exe, ns, datadir) to clean up
function Run {
    $bin, $ns, $dir, $rest = $args[0], $args[1], $args[2], @($args | Select-Object -Skip 3)
    $env:PSMUX_DATA_DIR = $dir
    $out = & $bin -L $ns @rest 2>&1 | Out-String
    return @{ code = $LASTEXITCODE; out = $out.Trim() }
}
function Wait-Session($bin, $ns, $dir, $s) {
    for ($i = 0; $i -lt 40; $i++) {
        $r = Run $bin $ns $dir has-session -t $s
        if ($r.code -eq 0) { return $true }
        Start-Sleep -Milliseconds 150
    }
    return $false
}
function Server-Pids($ns) {
    @(Get-CimInstance Win32_Process -Filter "Name LIKE '%.exe'" |
        Where-Object { $_.CommandLine -and $_.CommandLine -match " -L $ns( |$)" -and $_.CommandLine -match ' server ' } |
        ForEach-Object { $_.ProcessId })
}

# Binaries: a renamed copy, and a copy that keeps the psmux.exe name.
$binDir = Join-Path $root 'bin'; $stdDir = Join-Path $root 'std'
New-Item -ItemType Directory -Force $binDir, $stdDir | Out-Null
$renamed = Join-Path $binDir 'pr856_master.exe'
$standard = Join-Path $stdDir 'psmux.exe'
Copy-Item $exe $renamed -Force
Copy-Item $exe $standard -Force

# 1. A renamed binary reaches the server it started.
$ns1 = "${tag}_ren"; $d1 = Join-Path $root 'd_ren'; New-Item -ItemType Directory -Force $d1 | Out-Null
$started += , @($renamed, $ns1, $d1)
$r = Run $renamed $ns1 $d1 new-session -d -s x
Check 'renamed: new-session -d exits 0' ($r.code -eq 0) $r.out
Check 'renamed: has-session finds the session' (Wait-Session $renamed $ns1 $d1 x)
$r = Run $renamed $ns1 $d1 display-message -p -t x '#{session_name}:#{window_index}'
Check 'renamed: display-message answers' ($r.code -eq 0 -and $r.out -eq 'x:0') $r.out
$r = Run $renamed $ns1 $d1 ls
Check 'renamed: ls lists the session' ($r.code -eq 0 -and $r.out -match '^x:') $r.out
$r = Run $renamed $ns1 $d1 new-window -t x
$r2 = Run $renamed $ns1 $d1 display-message -p -t x '#{session_windows}'
Check 'renamed: new-window reaches the server' ($r2.out -eq '2') "$($r.out) / $($r2.out)"

# 2. A psmux.exe CLI sharing the data dir sees the renamed server and keeps it.
$r = Run $standard $ns1 $d1 has-session -t x
Check 'cross: psmux.exe CLI sees the renamed server' ($r.code -eq 0) $r.out
Check 'cross: registry files survive the psmux.exe CLI' (Test-Path (Join-Path $d1 "${ns1}__x.port"))
$r = Run $standard $ns1 $d1 new-session -d -s x
Check 'cross: duplicate new-session is refused as duplicate, not as a failed start' ($r.out -match 'duplicate') $r.out
$r = Run $renamed $ns1 $d1 has-session -t x
Check 'cross: renamed CLI still reaches its server afterwards' ($r.code -eq 0) $r.out

# 3. Renamed binary kill-server stops its own server.
$pids1 = Server-Pids $ns1
$r = Run $renamed $ns1 $d1 kill-server
Start-Sleep -Milliseconds 800
$alive = @($pids1 | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })
Check 'renamed: kill-server stops its server' ($alive.Count -eq 0) "alive=$($alive -join ',')"

# 4. A long data dir path (well past MAX_PATH once file names are appended).
$long = Join-Path $root (('L' * 60) + '\' + ('M' * 60) + '\' + ('N' * 60) + '\data')
New-Item -ItemType Directory -Force $long | Out-Null
$ns4 = "${tag}_long"; $started += , @($standard, $ns4, $long)
$r = Run $standard $ns4 $long new-session -d -s x
Check "long dir ($($long.Length) chars): new-session exits 0" ($r.code -eq 0) $r.out
Check 'long dir: has-session finds the session' (Wait-Session $standard $ns4 $long x)
$ns4r = "${tag}_longren"; $started += , @($renamed, $ns4r, $long)
$r = Run $renamed $ns4r $long new-session -d -s x
Check 'long dir + renamed: has-session finds the session' ($r.code -eq 0 -and (Wait-Session $renamed $ns4r $long x)) $r.out

# 5. A reused data dir whose previous server was killed hard.
$ns5 = "${tag}_reuse"; $d5 = Join-Path $root 'd_reuse'; New-Item -ItemType Directory -Force $d5 | Out-Null
$started += , @($standard, $ns5, $d5)
$null = Run $standard $ns5 $d5 new-session -d -s x
$null = Wait-Session $standard $ns5 $d5 x
foreach ($p in (Server-Pids $ns5)) { Stop-Process -Id $p -Force -ErrorAction SilentlyContinue }
Start-Sleep -Milliseconds 500
$r = Run $standard $ns5 $d5 new-session -d -s x
Check 'reused dir after hard kill: new-session exits 0' ($r.code -eq 0) $r.out
Check 'reused dir after hard kill: has-session finds the session' (Wait-Session $standard $ns5 $d5 x)

# Cleanup: namespace kill-server, then recorded server PIDs only.
foreach ($s in $started) { $null = Run $s[0] $s[1] $s[2] kill-server }
Start-Sleep -Milliseconds 800
foreach ($s in $started) { foreach ($p in (Server-Pids $s[1])) { Stop-Process -Id $p -Force -ErrorAction SilentlyContinue } }
Start-Sleep -Milliseconds 300
Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue

Write-Host ""
Write-Host "Passed: $pass  Failed: $fail"
if ($fail -gt 0) { exit 1 } else { exit 0 }
