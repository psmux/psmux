# Issue #865: exit codes must not be guessed from output text.
#
# The clearly safe part, compared with tmux 3.4 (WSL, same commands):
#   display-message -p "ERROR: m40"        tmux: stdout "ERROR: m40", exit 0
#   display-message -p -l "ERROR: #{x}"    printed as written, exit 0
#   list-windows -F "ERROR: m40"           stdout, exit 0 (already right)
#   display-message -t X:99 -p hi          tmux: "can't find window: 99", exit 1
#   list-windows -t X:99                   tmux: "can't find window: 99", exit 1 (#864)
#
# Isolation: unique -L namespace, private PSMUX_DATA_DIR, cleanup only with
# `-L <ns> kill-server`. PSMUX_TEST_EXE picks the binary.

$ErrorActionPreference = 'Continue'
$script:Passed = 0
$script:Failed = 0
function Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Passed++ }
function Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Failed++ }
function Check($ok, $m, $detail) { if ($ok) { Pass $m } else { Fail "$m -> $detail" } }

$Psmux = $env:PSMUX_TEST_EXE
if (-not $Psmux) { $f = Get-Command psmux -ErrorAction SilentlyContinue; if ($f) { $Psmux = $f.Source } }
if (-not $Psmux) { Write-Host "psmux binary not found"; exit 2 }
$Psmux = (Resolve-Path $Psmux).Path

$Tag = [guid]::NewGuid().ToString('N').Substring(0, 8)
$RootBase = if ($env:PSMUX_TEST_ROOT) { $env:PSMUX_TEST_ROOT } else { [System.IO.Path]::GetTempPath() }
$Root = Join-Path $RootBase ("psmux_865_" + $Tag)
$Ns = "t865" + $Tag
$SavedData = $env:PSMUX_DATA_DIR
$DefaultBefore = (& $Psmux ls 2>&1 | Out-String)
$env:PSMUX_DATA_DIR = Join-Path $Root "data"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
foreach ($n in 'TMUX', 'TMUX_PANE', 'PSMUX_SESSION', 'PSMUX_PANE', 'PSMUX_TARGET_SESSION', 'PSMUX_TARGET_FULL') {
    Remove-Item "Env:$n" -ErrorAction SilentlyContinue
}
$So = Join-Path $Root "so.txt"; $Se = Join-Path $Root "se.txt"
function Run { $a = $args
    & $Psmux -L $Ns @a 1>$So 2>$Se
    [pscustomobject]@{
        Code = $LASTEXITCODE
        Out = ([string](Get-Content $So -Raw) -replace "`r", '').TrimEnd("`n")
        Err = ([string](Get-Content $Se -Raw) -replace "`r", '').TrimEnd("`n")
    }
}

try {
    & $Psmux -L $Ns new-session -d -s X -x 120 -y 40 | Out-Null
    $dl = (Get-Date).AddSeconds(8)
    do { & $Psmux -L $Ns has-session -t X 2>$null; if ($LASTEXITCODE -eq 0) { break }; Start-Sleep -Milliseconds 150 } while ((Get-Date) -lt $dl)
    & $Psmux -L $Ns rename-window -t X:0 'ERROR: build failed' | Out-Null

    Write-Host "`n--- output that starts with ERROR is output ---" -ForegroundColor Yellow
    $r = Run display-message -t X -p "ERROR: m40"
    Check ($r.Code -eq 0 -and $r.Out -eq 'ERROR: m40' -and -not $r.Err) "display-message -t X -p 'ERROR: m40' prints it at exit 0" "[$($r.Code)] out '$($r.Out)' err '$($r.Err)'"
    $r = Run display-message -p "ERROR: m40"
    Check ($r.Code -eq 0 -and $r.Out -eq 'ERROR: m40') "display-message -p 'ERROR: m40' (no -t) prints it at exit 0" "[$($r.Code)] out '$($r.Out)' err '$($r.Err)'"
    $r = Run display-message -p "ERROR: can't find window: 9"
    Check ($r.Code -eq 0 -and $r.Out -eq "ERROR: can't find window: 9") "a literal message shaped like the server error prints at exit 0" "[$($r.Code)] out '$($r.Out)' err '$($r.Err)'"
    $r = Run display-message -t X -p -l 'ERROR: can''t find #{x}'
    Check ($r.Code -eq 0 -and $r.Out -eq "ERROR: can't find #{x}") "display-message -p -l prints the template as written at exit 0" "[$($r.Code)] out '$($r.Out)' err '$($r.Err)'"
    $r = Run display-message -t X -p '#{window_name}'
    Check ($r.Code -eq 0 -and $r.Out -eq 'ERROR: build failed') "a format that expands to ERROR text prints at exit 0" "[$($r.Code)] out '$($r.Out)' err '$($r.Err)'"
    $r = Run list-windows -t X -F "ERROR: m40"
    Check ($r.Code -eq 0 -and $r.Out -eq 'ERROR: m40') "list-windows -F 'ERROR: m40' prints it at exit 0" "[$($r.Code)] out '$($r.Out)' err '$($r.Err)'"

    Write-Host "`n--- real errors still fail ---" -ForegroundColor Yellow
    $r = Run display-message -t X:99 -p hi
    Check ($r.Code -eq 1 -and $r.Err -match "can't find window: 99" -and -not $r.Out) "display-message -t X:99 -p fails" "[$($r.Code)] out '$($r.Out)' err '$($r.Err)'"
    $r = Run display-message -t X:0.99 -p hi
    Check ($r.Code -eq 1 -and $r.Err -match "can't find pane: 99" -and -not $r.Out) "display-message -t X:0.99 -p fails" "[$($r.Code)] out '$($r.Out)' err '$($r.Err)'"
    $r = Run display-message -t '%999' -p hi
    Check ($r.Code -eq 1 -and $r.Err -match "can't find pane: %999") "display-message -t %999 -p fails" "[$($r.Code)] out '$($r.Out)' err '$($r.Err)'"
    $r = Run list-windows -t X:99
    Check ($r.Code -eq 1 -and $r.Err -match "can't find window: 99" -and -not $r.Out) "list-windows -t X:99 fails (#864)" "[$($r.Code)] out '$($r.Out)' err '$($r.Err)'"
    $r = Run display-message -t X -p hello
    Check ($r.Code -eq 0 -and $r.Out -eq 'hello') "control: display-message -p hello" "[$($r.Code)] out '$($r.Out)' err '$($r.Err)'"
}
catch { Fail "setup or unexpected error: $_" }
finally {
    & $Psmux -L $Ns kill-server 2>&1 | Out-Null
    $env:PSMUX_DATA_DIR = $SavedData
    $DefaultAfter = (& $Psmux ls 2>&1 | Out-String)
    Check ($DefaultAfter -ceq $DefaultBefore) "default namespace ls unchanged" "before '$DefaultBefore' after '$DefaultAfter'"
    Remove-Item -Recurse -Force $Root -ErrorAction SilentlyContinue
}

Write-Host "`n  Passed: $script:Passed  Failed: $script:Failed"
if ($script:Failed -gt 0) { exit 1 }
exit 0
