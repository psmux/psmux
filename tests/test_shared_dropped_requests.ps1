# shared (#798): an attached client's binding that the shared server dropped.
# prefix Space (next-layout) and prefix ] (paste-buffer) did nothing in shared
# mode, and prefix ! (break-pane, still not run there) gave no word either.
# Now the first two run, list-buffers runs as a one shot command, and the
# refused one says so on the status line.
#
# Isolation: a unique -L namespace, a private PSMUX_DATA_DIR, an empty config;
# cleanup only with `-L <ns> kill-server` and the client window by PID.
# Pass -Exe or set PSMUX_TEST_EXE; -Mode default runs the same steps on a per
# session server, where every check but the refusal must hold too.
param([string]$Exe = $(if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }),
      [ValidateSet("shared", "default")][string]$Mode = "shared")

$ErrorActionPreference = "Continue"
$script:Passed = 0
$script:Failed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Passed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Failed++ }

$injector = Join-Path $env:TEMP "psmux_shared_drops_injector.exe"
if (-not (Test-Path $injector)) {
    $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    & $csc /nologo /optimize "/out:$injector" (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
}

$saved = @{ PSMUX_DATA_DIR = $env:PSMUX_DATA_DIR; PSMUX_SERVER_MODE = $env:PSMUX_SERVER_MODE; PSMUX_CONFIG_FILE = $env:PSMUX_CONFIG_FILE }
Remove-Item Env:\PSMUX_DATA_DIR -EA SilentlyContinue
$defaultBefore = (& $Exe ls 2>&1 | Out-String).Trim()

$ns = "shdrops$(Get-Random)"
$data = Join-Path $env:TEMP "psmux_$ns"
New-Item -ItemType Directory $data -Force | Out-Null
$env:PSMUX_DATA_DIR = $data
$empty = Join-Path $data "empty.conf"
"" | Set-Content $empty
$env:PSMUX_CONFIG_FILE = $empty
if ($Mode -eq "shared") { $env:PSMUX_SERVER_MODE = "shared" } else { Remove-Item Env:\PSMUX_SERVER_MODE -EA SilentlyContinue }
function P { & $Exe -L $ns @args 2>&1 }

Write-Host "`n=== shared dropped requests ($Mode) ===" -ForegroundColor Cyan
$proc = $null
try {
    P new-session -d -s A -x 120 -y 30 | Out-Null
    P new-session -d -s B -x 120 -y 30 | Out-Null
    Start-Sleep -Milliseconds 1500
    P split-window -t A | Out-Null
    Start-Sleep -Milliseconds 800
    P set-buffer -t A "DROPS_MARKER_42" | Out-Null
    P set -g display-time 5000 | Out-Null
    $pa = (P display -p -t A '#{pid}') -join ''
    $pb = (P display -p -t B '#{pid}') -join ''
    if ($Mode -eq "shared") {
        if ($pa -and $pa -eq $pb) { Write-Pass "A and B share one server (pid $pa)" } else { Write-Fail "not one shared server: A=$pa B=$pb" }
    }

    $buffers = (P list-buffers -t A) -join ' | '
    if ($buffers -match 'DROPS_MARKER_42') { Write-Pass "list-buffers runs as a one shot command" } else { Write-Fail "list-buffers: $buffers" }

    $proc = Start-Process -FilePath $Exe -ArgumentList "-L", $ns, "attach", "-t", "A" -PassThru
    Start-Sleep -Seconds 3

    $l0 = (P display -p -t A '#{window_layout}') -join ''
    & $injector $proc.Id "^b{SLEEP:400} " | Out-Null
    Start-Sleep -Milliseconds 1200
    $l1 = (P display -p -t A '#{window_layout}') -join ''
    if ($l0 -and $l0 -ne $l1) { Write-Pass "prefix Space runs next-layout ($l0 -> $l1)" } else { Write-Fail "prefix Space left the layout at $l1" }

    & $injector $proc.Id "^b{SLEEP:400}]" | Out-Null
    Start-Sleep -Milliseconds 1500
    $cap = (P capture-pane -p -t A) -join "`n"
    if ($cap -match 'DROPS_MARKER_42') { Write-Pass "prefix ] pastes the buffer into the pane" } else { Write-Fail "prefix ] pasted nothing" }

    if ($Mode -eq "shared") {
        $w0 = (P display -p -t A '#{session_windows}') -join ''
        & $injector $proc.Id "^b{SLEEP:400}!" | Out-Null
        Start-Sleep -Milliseconds 1200
        $w1 = (P display -p -t A '#{session_windows}') -join ''
        $dump = (P dump-state -t A) -join ''
        $sm = if ($dump -match '"status_message":"([^"]*)"') { $Matches[1] } else { '' }
        if ($sm -eq 'not supported in shared server mode yet: break-pane') { Write-Pass "prefix ! says break-pane is not supported yet" } else { Write-Fail "prefix ! status message: '$sm'" }
        if ($w0 -eq $w1) { Write-Pass "the refused break-pane changed nothing ($w0 windows)" } else { Write-Fail "windows $w0 -> $w1" }
        $dumpB = (P dump-state -t B) -join ''
        if ($dumpB -notmatch 'not supported in shared server mode yet') { Write-Pass "session B is not told" } else { Write-Fail "B got A's message" }
    }
} finally {
    P kill-server | Out-Null
    if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue }
    foreach ($k in $saved.Keys) {
        if ($null -eq $saved[$k]) { Remove-Item "Env:\$k" -EA SilentlyContinue } else { Set-Item "Env:\$k" $saved[$k] }
    }
    Remove-Item -Recurse -Force $data -EA SilentlyContinue
}

Remove-Item Env:\PSMUX_DATA_DIR -EA SilentlyContinue
$defaultAfter = (& $Exe ls 2>&1 | Out-String).Trim()
if ($null -ne $saved.PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR = $saved.PSMUX_DATA_DIR }
if ($defaultAfter -eq $defaultBefore) { Write-Pass "the default namespace is unchanged" } else { Write-Fail "default namespace changed: '$defaultBefore' -> '$defaultAfter'" }

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:Passed)"
Write-Host "  Failed: $($script:Failed)"
exit $script:Failed
