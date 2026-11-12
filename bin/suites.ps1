param([string]$Exe, [string]$Tag, [string]$Suites = '')
$tests = 'C:\Users\godwin\Documents\workspace\psmux-pr1008\tests'
$out = "C:\Users\godwin\Documents\workspace\psmux-pr1008\bin\suites_$Tag.txt"
"" | Set-Content $out
Remove-Item Env:SHELL -EA SilentlyContinue
$env:PSMUX_CONPTY_DIR_FOR_TEST = 'C:\Users\godwin\AppData\Local\Temp\occ-conpty\nugetstable'
$list = if ($Suites) { $Suites -split ',' } else { @('test_issue685_pane_palette.ps1', 'test_issue980_pane_colours.ps1', 'test_issue941_pane_reply_queue.ps1', 'test_issue930_pane_reply_order.ps1') }
foreach ($t in $list) {
    $env:PSMUX_BIN = $Exe; $env:PSMUX_TEST_EXE = $Exe
    $log = "C:\Users\godwin\Documents\workspace\psmux-pr1008\bin\${Tag}_$t.log"
    pwsh -NoProfile -File (Join-Path $tests $t) *> $log
    $code = $LASTEXITCODE
    $summary = (Select-String -Path $log -Pattern 'passed|Passed|PASS:|Results|FAIL' | Select-Object -Last 3 | ForEach-Object { $_.Line.Trim() }) -join ' | '
    "$t exit=$code :: $summary" | Add-Content $out
}
