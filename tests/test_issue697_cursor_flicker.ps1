# Issue #697 item 2: "Execute :FzfLua live_grep in NeoVim inside psmux, start
# to type something (e.g. 'aaaaaaaa' with 0.3s pauses between each character).
# The cursor flickers; it does not flicker in NeoVim without psmux."
#
# A cursor flicker is the host terminal SHOWING its cursor while it travels:
# text printed or the cursor moved between ESC[?25h and the next ESC[?25l. So
# this script hosts the real thing inside a CreatePseudoConsole
# (tests/conpty697.cs), exactly the way Windows Terminal hosts psmux, types
# eight 'a's 300 ms apart into live_grep, and counts per keystroke the runs of
# drawing done while the cursor is visible ("visible draws").
#
# Measured before the fix (psmux a7ab8ea): 1 to 3 visible draws on EVERY
# keystroke, e.g.
#     ESC[?25h a ESC[78C <spinner> ESC[?25l ESC[4;19H ESC[?25h
# the cursor running 78 columns to fzf's spinner and back, because ratatui
# drew the cells first and hid the cursor after them. Neovim alone: 0 on every
# keystroke. After the fix: 0 on every keystroke, like Neovim alone.
#
# The byte policy itself is covered with no server and no Neovim by
# tests-rs/test_issue697_cursor_flicker.rs.
#
# Needs nvim, fzf, rg and fzf-lua (cloned with git when absent). Skips cleanly
# with a reason when any of them is missing.

$ErrorActionPreference = "Continue"

$SOCK = "i697"
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red;   $script:TestsFailed++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Test($m) { Write-Host "`n[$m]" -ForegroundColor Cyan }
function Finish {
    Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed, $($script:TestsSkipped) skipped ==="
    if ($script:TestsFailed -gt 0) { exit 1 }
    exit 0
}

$PSMUX = $env:PSMUX_TEST_EXE
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { $PSMUX = (Get-Command psmux -EA SilentlyContinue).Source }
if (-not $PSMUX) { Write-Host "psmux not found"; exit 1 }
Write-Host "  [INFO] binary under test: $PSMUX"

# --- prerequisites -----------------------------------------------------------
$links = Join-Path $env:LOCALAPPDATA "Microsoft\WinGet\Links"
if (Test-Path $links) { $env:PATH = "$links;$env:PATH" }
if (Test-Path "C:\Program Files\Neovim\bin") { $env:PATH = "C:\Program Files\Neovim\bin;$env:PATH" }
Write-Test "prerequisites"
foreach ($tool in "nvim", "fzf", "rg") {
    if (-not (Get-Command $tool -EA SilentlyContinue)) {
        Write-Skip "$tool is not installed on this machine, so live_grep cannot run (winget install Neovim.Neovim junegunn.fzf BurntSushi.ripgrep.MSVC)"
        Finish
    }
}

$work = Join-Path $env:TEMP "psmux_i697"
New-Item -ItemType Directory -Force -Path $work | Out-Null

# fzf-lua: reuse a clone, else clone one into the work dir.
$fzfLua = $null
foreach ($c in @(
        (Join-Path $work "fzf-lua"),
        (Join-Path $env:LOCALAPPDATA "psmux697-data\site\pack\plugins\start\fzf-lua"),
        (Join-Path $env:LOCALAPPDATA "nvim-data\lazy\fzf-lua"))) {
    if (Test-Path (Join-Path $c "lua\fzf-lua\init.lua")) { $fzfLua = $c; break }
}
if (-not $fzfLua -and (Get-Command git -EA SilentlyContinue)) {
    & git clone --quiet --depth 1 https://github.com/ibhagwan/fzf-lua (Join-Path $work "fzf-lua") 2>&1 | Out-Null
    if (Test-Path (Join-Path $work "fzf-lua\lua\fzf-lua\init.lua")) { $fzfLua = Join-Path $work "fzf-lua" }
}
if (-not $fzfLua) { Write-Skip "fzf-lua is not available and could not be cloned"; Finish }

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { Write-Skip "csc.exe not found, cannot build the pseudoconsole harness"; Finish }
$harness = Join-Path $work "conpty697.exe"
& $csc /nologo /optimize /out:$harness (Join-Path $PSScriptRoot "conpty697.cs") 2>&1 | Out-Null
if (-not (Test-Path $harness)) { Write-Skip "the pseudoconsole harness did not compile"; Finish }
Write-Pass "nvim, fzf, rg, fzf-lua ($fzfLua) and the harness are present"

# Isolated Neovim: its own XDG roots, only fzf-lua on the runtimepath.
$xdgCfg = Join-Path $work "xdg_config"
$xdgData = Join-Path $work "xdg_data"
New-Item -ItemType Directory -Force -Path (Join-Path $xdgCfg "psmux697"), $xdgData | Out-Null
$luaPath = ($fzfLua -replace '\\', '/')
@"
vim.opt.shadafile = "NONE"
vim.opt.swapfile = false
vim.opt.rtp:prepend("$luaPath")
require("fzf-lua").setup({})
"@ | Set-Content -Path (Join-Path $xdgCfg "psmux697\init.lua") -Encoding ASCII
$env:XDG_CONFIG_HOME = $xdgCfg
$env:XDG_DATA_HOME = $xdgData
$env:NVIM_APPNAME = "psmux697"

# A small tree for live_grep to search.
$tree = Join-Path $work "tree"
New-Item -ItemType Directory -Force -Path $tree | Out-Null
1..5 | ForEach-Object { Set-Content (Join-Path $tree "file$_.txt") ("aaaa line $_`nbbb aaaaaa`nhello aaaaaaaa world`n" * 3) }

function New-Script([int]$settleMs) {
    $l = @("WAIT $settleMs", "MARK open", "TEXT :FzfLua live_grep", "WAIT 500", "CR", "WAIT 3000")
    for ($k = 1; $k -le 8; $k++) { $l += "MARK k$k"; $l += "TEXT a"; $l += "WAIT 300" }
    $l += "WAIT 1200"; $l += "MARK quit"; $l += "ESC"; $l += "WAIT 300"; $l += "TEXT :qa!"; $l += "WAIT 500"; $l += "CR"; $l += "WAIT 800"; $l += "END"
    $p = Join-Path $work "script_$settleMs.txt"
    $l | Set-Content -Path $p -Encoding ASCII
    return $p
}

# Visible draws: runs of printing or cursor movement while the cursor is shown.
function Measure-Capture([string]$out) {
    $bytes = [IO.File]::ReadAllBytes($out)
    $text = [Text.Encoding]::GetEncoding(28591).GetString($bytes)
    $marks = @()
    foreach ($l in Get-Content "$out.idx") {
        if ($l -match '^MARK (\S+) (\d+) ') { $marks += [pscustomobject]@{ Name = $Matches[1]; Off = [int]$Matches[2] } }
    }
    $vis = $true
    $rows = @()
    $tok = [regex]"`e\[[\x30-\x3f]*[\x20-\x2f]*[\x40-\x7e]|`e\][^`a`e]*(`a|`e\\)|`e.|[\s\S]"
    for ($i = 0; $i -lt $marks.Count - 1; $i++) {
        $a = $marks[$i].Off; $b = $marks[$i + 1].Off
        $seg = $text.Substring($a, $b - $a)
        $n = 0; $inRun = $false; $sample = ""
        foreach ($m in $tok.Matches($seg)) {
            $t = $m.Value
            if ($t -eq "`e[?25l") { $vis = $false; $inRun = $false; continue }
            if ($t -eq "`e[?25h") { $vis = $true; $inRun = $false; continue }
            if (-not $vis) { continue }
            $draw = ($t[0] -ne [char]27) -or ($t -match "^`e\[\d*(;\d*)?[HABCDGfd]$")
            if ($draw -and -not $inRun) {
                $n++; $inRun = $true
                if (-not $sample) {
                    $st = [Math]::Max(0, $m.Index - 30)
                    $sample = ($seg.Substring($st, [Math]::Min(90, $seg.Length - $st)) -replace "`e", '\e' -replace "`r", '\r' -replace "`n", '\n')
                }
            }
        }
        $rows += [pscustomobject]@{
            Seg = $marks[$i].Name; Bytes = $b - $a
            Hide = ([regex]::Matches($seg, "`e\[\?25l")).Count
            Show = ([regex]::Matches($seg, "`e\[\?25h")).Count
            VisDraw = $n; Sample = $sample
        }
    }
    return [pscustomobject]@{ Rows = $rows; Text = $text }
}

function Invoke-Capture([string]$name, [string]$cmd, [int]$settleMs) {
    $out = Join-Path $work "$name.bin"
    Remove-Item $out, "$out.idx" -Force -EA SilentlyContinue
    $script = New-Script $settleMs
    Push-Location $tree
    try {
        # The harness runs its script to END and exits on its own (~20 s).
        & $harness $script $out 120 30 0 $cmd
    } finally { Pop-Location }
    if (-not (Test-Path "$out.idx")) { return $null }
    return Measure-Capture $out
}

function Show-Table($r) {
    $r.Rows | Where-Object { $_.Seg -match '^k\d$' } |
        ForEach-Object { Write-Host ("    {0,-3} bytes={1,5} hide={2} show={3} visible_draws={4} {5}" -f $_.Seg, $_.Bytes, $_.Hide, $_.Show, $_.VisDraw, $(if ($_.VisDraw) { "<< " + $_.Sample } else { "" })) }
}

# --- baseline: Neovim alone --------------------------------------------------
Write-Test "baseline: nvim alone in the pseudoconsole"
$nv = Invoke-Capture "nvim_alone" "nvim" 3000
if (-not $nv) { Write-Skip "the harness produced no capture for nvim alone"; Finish }
if ($nv.Text -notmatch 'file\d\.txt:\d+:\d+:') {
    Write-Skip "live_grep showed no results in Neovim alone, so this machine cannot reproduce the flow"
    Finish
}
Show-Table $nv
$nvKeys = @($nv.Rows | Where-Object { $_.Seg -match '^k\d$' })
$nvVis = ($nvKeys | Measure-Object VisDraw -Sum).Sum
Write-Host "  [INFO] nvim alone: $nvVis visible draws over $($nvKeys.Count) keystrokes"

# --- psmux ---------------------------------------------------------------------
Write-Test "nvim inside psmux (attached client in the pseudoconsole)"
$sess = "i697_$PID"
& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
$px = Invoke-Capture "nvim_psmux" "`"$PSMUX`" -L $SOCK new-session -s $sess nvim" 5000
& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
if (-not $px) { Write-Fail "the harness produced no capture for psmux"; Finish }
if ($px.Text -notmatch 'file\d\.txt:\d+:\d+:') {
    Write-Fail "live_grep showed no results inside psmux, the flow did not run"
    Finish
}
Show-Table $px
$pxKeys = @($px.Rows | Where-Object { $_.Seg -match '^k\d$' })
$active = @($pxKeys | Where-Object { $_.Bytes -gt 0 }).Count
if ($active -ge 6) { Write-Pass "$active of $($pxKeys.Count) keystrokes redrew the client" }
else { Write-Fail "only $active of $($pxKeys.Count) keystrokes produced output, the keys did not reach live_grep" }

$pxVis = ($pxKeys | Measure-Object VisDraw -Sum).Sum
$worst = ($pxKeys | Measure-Object VisDraw -Maximum).Maximum
if ($pxVis -eq 0) {
    Write-Pass "no keystroke drew while the host cursor was visible (nvim alone: $nvVis)"
} else {
    Write-Fail "$pxVis visible draws over $($pxKeys.Count) keystrokes, up to $worst on one (nvim alone: $nvVis): the cursor flickers (#697)"
}

$shows = ($pxKeys | Measure-Object Show -Sum).Sum
$hides = ($pxKeys | Measure-Object Hide -Sum).Sum
Write-Host "  [INFO] psmux per 8 keystrokes: hide=$hides show=$shows"

Finish
