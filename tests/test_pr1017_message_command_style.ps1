# PR #1017: message-command-style. tmux draws the command prompt with
# message-style, and with message-command-style only while the prompt is in vi
# command mode (prompt_effective_style, prompt.c:438-450 at tag 3.8; in 3.5a
# status_prompt_redraw, status.c:742-745). Escape enters command mode when
# status-keys is vi (prompt.c:1265 and :819); with emacs keys Escape closes
# the prompt. psmux drew the `:` prompt in the terminal's default colours and
# had no command mode, and the PR as sent drew every prompt with
# message-command-style.
#
# A real client in its own console; keys by WriteConsoleInput
# (tests\injector.cs); the screen and its colours read back with
# tests\console_screen_read.cs. message-style is bg=green (legacy attribute
# background 0x20), message-command-style bg=red (0x40).
#
# Private PSMUX_DATA_DIR and config, a -L namespace per run; cleanup is
# `-L <ns> kill-server` and the client PID this suite started.
# PSMUX_EXE picks the binary (defaults to target\release\psmux.exe).

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -and $_.Name -ne 'PSMUX_EXE' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
Remove-Item Env:SHELL -EA SilentlyContinue
$PSMUX = $env:PSMUX_EXE
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Check($name, $got, $want) {
    if ("$got" -ceq "$want") { Write-Pass "$name" } else { Write-Fail "$name`n      expected: [$want]`n      got:      [$got]" }
}

$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_mcs_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null
$env:PSMUX_DATA_DIR = $TMP
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_RUN_NOPAUSE = "1"
$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
$INJ = Join-Path $TMP "injector.exe"
$READ = Join-Path $TMP "screenread.exe"
if ($csc) {
    & $csc /nologo /optimize "/out:$INJ" (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
    & $csc /nologo /optimize "/out:$READ" (Join-Path $PSScriptRoot "console_screen_read.cs") 2>&1 | Out-Null
}
if (-not ((Test-Path $INJ) -and (Test-Path $READ))) { Write-Error "could not build tests\injector.cs or tests\console_screen_read.cs"; exit 1 }
$POWERSHELL = (Get-Command pwsh -EA SilentlyContinue).Source
if (-not $POWERSHELL) { $POWERSHELL = (Get-Command powershell).Source }

$script:client = $null
$script:ns = ""
function Start-Run($keys) {
    $script:ns = "mcs_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    $conf = Join-Path $TMP "$($script:ns).conf"
    @("set -g status-keys $keys", "set -g message-style bg=green,fg=black", "set -g message-command-style bg=red,fg=white",
      "set -g default-shell '$POWERSHELL -NoLogo -NoProfile'") -join "`n" | Set-Content $conf -Encoding ASCII
    $env:PSMUX_CONFIG_FILE = $conf
    & $PSMUX -L $script:ns new-session -d -s A -x 100 -y 25 2>&1 | Out-Null
    for ($i = 0; $i -lt 40; $i++) { & $PSMUX -L $script:ns has-session -t A 2>$null; if ($LASTEXITCODE -eq 0) { break }; Start-Sleep -Milliseconds 250 }
    Start-Sleep -Milliseconds 800
    $script:client = Start-Process -FilePath $PSMUX -ArgumentList @('-L', $script:ns, 'attach', '-t', 'A') -PassThru
    Start-Sleep -Seconds 4
}
function Stop-Run {
    if ($script:ns) { & $PSMUX -L $script:ns kill-server 2>&1 | Out-Null }
    Start-Sleep -Milliseconds 500
    if ($script:client -and -not $script:client.HasExited) { Stop-Process -Id $script:client.Id -Force -EA SilentlyContinue }
    $script:client = $null; $script:ns = ""
}
function Send($keys) { & $INJ $script:client.Id $keys | Out-Null; Start-Sleep -Milliseconds 900 }
# The prompt as drawn: its text after ": " and the background of its cells
# (high nibble of the legacy attribute), or "closed" when no prompt is drawn.
function Prompt {
    $f = Join-Path $TMP "screen.txt"
    & $READ $script:client.Id $f | Out-Null
    $lines = @(Get-Content $f -Encoding UTF8)
    $T = @($lines | Where-Object { $_.StartsWith('T|') }); $A = @($lines | Where-Object { $_.StartsWith('A|') })
    for ($y = 0; $y -lt $T.Count; $y++) {
        $row = $T[$y].Substring(2)
        $m = [regex]::Match($row, '│: (.*?)\s*│')
        if ($m.Success) {
            $attrs = $A[$y].Substring(2).Trim() -split ' '
            $bg = '{0:X2}' -f ([Convert]::ToInt32($attrs[$m.Index + 1], 16) -band 0xF0)
            return @{ text = $m.Groups[1].Value; bg = $bg }
        }
    }
    return @{ text = "closed"; bg = "" }
}
$GREEN = "20"; $RED = "40"

Write-Host "`n=== PR #1017 message-command-style ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"

try {
    Write-Host "`n[1] status-keys vi" -ForegroundColor Yellow
    Start-Run vi
    Send "^b{SLEEP:500}:{SLEEP:400}abc def"
    $p = Prompt
    Check "the prompt is open with what was typed" $p.text "abc def"
    Check "insert mode draws with message-style (green)" $p.bg $GREEN
    Send "{ESC}"
    $p = Prompt
    Check "Escape keeps the prompt open" $p.text "abc def"
    Check "command mode draws with message-command-style (red)" $p.bg $RED
    Send "0"; Send "x"
    Check "0 then x deletes the first character" (Prompt).text "bc def"
    Send "w"; Send "D"
    Check "w then D cuts to the end of the line" (Prompt).text "bc"
    Send "A"
    $p = Prompt
    Check "A goes back to insert mode (green)" $p.bg $GREEN
    Send "zz"
    Check "and types at the end" (Prompt).text "bc zz"
    Send "{ESC}"; Send "i"
    Check "i leaves command mode (green)" (Prompt).bg $GREEN
    Send "{ESC}"; Send "q"
    Check "q in command mode closes the prompt" (Prompt).text "closed"
    Send "^b{SLEEP:500}:{SLEEP:400}xy"
    Check "a new prompt opens in insert mode (green)" (Prompt).bg $GREEN
    Send "{ESC}"; Send "{ESC}"
    Check "a second Escape stays in command mode" (Prompt).bg $RED
    Send "S"
    $p = Prompt
    Check "S clears the line and goes to insert mode" "$($p.text)|$($p.bg)" "|$GREEN"
    Send "display-message -p hi{ENTER}"
    Check "Enter runs the line and closes the prompt" (Prompt).text "closed"
    # The option follows a change while the client is attached.
    & $PSMUX -L $script:ns set -g status-keys emacs 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    Send "^b{SLEEP:500}:{SLEEP:400}ab"
    Send "{ESC}"
    Check "after set status-keys emacs, Escape closes the prompt" (Prompt).text "closed"
    Stop-Run

    Write-Host "`n[2] status-keys emacs (the default)" -ForegroundColor Yellow
    Start-Run emacs
    Send "^b{SLEEP:500}:{SLEEP:400}abc"
    $p = Prompt
    Check "the prompt draws with message-style (green), not message-command-style" $p.bg $GREEN
    Send "{ESC}"
    Check "Escape closes the prompt" (Prompt).text "closed"
    Stop-Run
}
catch {
    Write-Fail "exception: $_"
}
finally {
    Stop-Run
    Remove-Item -Recurse -Force $TMP -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
