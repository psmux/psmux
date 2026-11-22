# PR #575: mouse-selection-force option.
# Claims:
#   - default off: pane app with mouse tracking owns the whole gesture (unchanged, PR #479)
#   - force on: drags are consumed by psmux (drag-to-copy works), the app sees no drag
#   - force on: a plain click (no movement) is replayed to the app as press+release
#   - wheel keeps reaching the app either way
# The mouse-aware app is REAL nvim with `set mouse=a` (a genuine VT writer: ConPTY
# eats DECSET from WriteConsole children like node, so wants_mouse would stay
# false with a sim; nvim's 1002h reaches the pane parser).  nvim maps every
# mouse event to append a line to a log file, giving byte-level proof of what
# the application received.
$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$NVIM = (Get-Command nvim -EA SilentlyContinue).Source
if (-not $NVIM) { $NVIM = "C:\Program Files\Neovim\bin\nvim.exe" }
if (-not (Test-Path $NVIM)) { Write-Host "SKIP: nvim not found" -ForegroundColor Yellow; exit 0 }
$psmuxDir = "$env:USERPROFILE\.psmux"
$S = "tpr575"
$script:Pass = 0; $script:Fail = 0
# The Windows 10 inbox conhost (build 19045) keeps nvim's ?1002h to itself, so
# psmux is never told that nvim wants the mouse, and it drops an SGR report
# written to the pane, so nvim gets no mouse from psmux at all (Issue #1089,
# docs/faq.md; measured with SGR reports typed with send-keys -l into a raw VT
# reader, which got only the letters around them). So there the pane does not
# report wants_mouse, a plain drag is psmux's selection, and nvim logs nothing.
$Win10 = [Environment]::OSVersion.Version.Build -lt 22000
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }

# compile injectors
$DRAG = "$env:LOCALAPPDATA\Temp\psmux_mouse_drag_injector.exe"
$WHEEL = "$env:LOCALAPPDATA\Temp\psmux_mouse_injector.exe"
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $DRAG)) { & $csc /nologo /optimize /out:$DRAG "$PSScriptRoot\mouse_drag_injector.cs" 2>&1 | Out-Null }
if (-not (Test-Path $WHEEL)) { & $csc /nologo /optimize /out:$WHEEL "$PSScriptRoot\mouse_injector.cs" 2>&1 | Out-Null }
if (-not (Test-Path $DRAG)) { Write-Host "FATAL: drag injector failed to compile"; exit 1 }

# nvim setup: content file + mouse-event logging mappings
$simDir = Join-Path $env:LOCALAPPDATA "Temp\psmux_pr575_sim"
New-Item -ItemType Directory -Force -Path $simDir | Out-Null
$mlog = (Join-Path $simDir "nvim-mouse-log.txt") -replace '\\','/'
$content = Join-Path $simDir "content.txt"
$setup = Join-Path $simDir "setup.vim"
if (Test-Path $mlog) { Remove-Item $mlog -Force }
(1..35 | ForEach-Object { "SELROW$($_.ToString('00')) the quick brown fox jumps over" }) | Set-Content -Path $content -Encoding ASCII
@"
set mouse=a
let g:mlog = '$mlog'
nnoremap <LeftMouse> <Cmd>call writefile(['press'], g:mlog, 'a')<CR>
nnoremap <LeftDrag> <Cmd>call writefile(['drag'], g:mlog, 'a')<CR>
nnoremap <LeftRelease> <Cmd>call writefile(['release'], g:mlog, 'a')<CR>
nnoremap <ScrollWheelUp> <Cmd>call writefile(['wheelup'], g:mlog, 'a')<CR>
nnoremap <ScrollWheelDown> <Cmd>call writefile(['wheeldown'], g:mlog, 'a')<CR>
"@ | Set-Content -Path $setup -Encoding ASCII

# classic conhost delegation
$sk = "HKCU:\Console\%%Startup"; if (-not (Test-Path $sk)) { New-Item -Path $sk -Force | Out-Null }
$oDC = (Get-ItemProperty $sk -EA SilentlyContinue).DelegationConsole; $oDT = (Get-ItemProperty $sk -EA SilentlyContinue).DelegationTerminal
$classic = "{B23D10C0-E52E-411E-9D5B-C09FDF709C7D}"
Set-ItemProperty $sk -Name DelegationConsole -Value $classic; Set-ItemProperty $sk -Name DelegationTerminal -Value $classic
function Restore { if ($oDC) { Set-ItemProperty $sk -Name DelegationConsole -Value $oDC } else { Remove-ItemProperty $sk -Name DelegationConsole -EA SilentlyContinue }; if ($oDT) { Set-ItemProperty $sk -Name DelegationTerminal -Value $oDT } else { Remove-ItemProperty $sk -Name DelegationTerminal -EA SilentlyContinue } }

# Set-Clipboard and Get-Clipboard fail quietly while another window holds the
# clipboard open, so a set is checked and a read is retried.
function Set-ClipboardVerified([string]$Value) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 5000) {
        try { Set-Clipboard -Value $Value -EA Stop } catch {}
        Start-Sleep -Milliseconds 100
        $now = try { (Get-Clipboard -Raw -EA Stop) } catch { $null }
        if ($null -ne $now -and $now.TrimEnd("`r", "`n") -eq $Value) { return $true }
    }
    return $false
}
function Read-Clipboard {
    for ($i = 0; $i -lt 20; $i++) {
        $now = try { (Get-Clipboard -Raw -EA Stop) } catch { $null }
        if ($null -ne $now -and $now -ne '') { return $now.TrimEnd("`r", "`n") }
        Start-Sleep -Milliseconds 100
    }
    return ''
}
function Read-MLog { if (Test-Path $mlog) { (Get-Content $mlog -EA SilentlyContinue) -join "," } else { "" } }
function Clear-MLog { if (Test-Path $mlog) { Remove-Item $mlog -Force -EA SilentlyContinue } }

Write-Host "=== PR #575: mouse-selection-force (nvim mouse=a pane) ===" -ForegroundColor Cyan
& $PSMUX kill-session -t $S 2>&1 | Out-Null; Start-Sleep -Milliseconds 600
$conhost = "$env:WINDIR\System32\conhost.exe"
$nvimCmd = "`"$NVIM`" -u NONE -i NONE -S `"$setup`" `"$content`""
$proc = Start-Process -FilePath $conhost -ArgumentList $PSMUX,"new-session","-s",$S,$nvimCmd -PassThru
Start-Sleep -Seconds 7
$child = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($proc.Id)" | Where-Object { $_.Name -like 'psmux*' } | Select-Object -First 1
$cpid = if ($child) { [int]$child.ProcessId } else { $proc.Id }

& $PSMUX set-option -g mouse on -t $S 2>&1 | Out-Null
Start-Sleep -Seconds 2

# option exists as boolean default off
$optlist = (& $PSMUX show-options -g -t $S 2>&1 | Out-String)
if ($optlist -match "mouse-selection-force off") { Write-Pass "option exists, defaults off" }
else { Write-Fail "mouse-selection-force not listed as boolean default off" }

# sanity: nvim's mouse protocol is visible (wants_mouse in dump)
$port = (Get-Content "$psmuxDir\$S.port" -Raw).Trim(); $key = (Get-Content "$psmuxDir\$S.key" -Raw).Trim()
$tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port); $tcp.NoDelay = $true; $tcp.ReceiveTimeout = 4000
$st = $tcp.GetStream(); $w = [System.IO.StreamWriter]::new($st); $r = [System.IO.StreamReader]::new($st)
$w.Write("AUTH $key`n"); $w.Flush(); $null = $r.ReadLine(); $w.Write("dump-state`n"); $w.Flush()
$best = $null; for ($j = 0; $j -lt 60; $j++) { try { $l = $r.ReadLine() } catch { break }; if ($null -eq $l) { break }; if ($l -ne "NC" -and $l.Length -gt 100) { $best = $l; break } }
$tcp.Close()
if ($Win10) {
    if ($best -and $best -notmatch '"wants_mouse"\s*:\s*true') { Write-Pass "on this host the pane does not report wants_mouse: nvim's 1002h never reached psmux" }
    else { Write-Fail "the pane reports wants_mouse=true, but this host was measured to pass no DECSET" }
}
elseif ($best -match '"wants_mouse"\s*:\s*true') { Write-Pass "pane reports wants_mouse=true (nvim 1002h visible)" }
else { Write-Fail "pane does not report wants_mouse=true; mouse-aware gating cannot be tested" }

# --- Case A: force OFF (default): app owns the drag (PR #479 behavior) ---
Write-Host "`n[Case A] force off: nvim owns the drag" -ForegroundColor Yellow
if (-not (Set-ClipboardVerified "CLIP_SENTINEL_575")) { Write-Host "  [INFO] the clipboard did not take the sentinel" }
Clear-MLog
& $DRAG $cpid drag 2 5 25 5 6 60 | Out-Null
Start-Sleep -Seconds 2
$logA = Read-MLog
$bufA = (& $PSMUX show-buffer -t $S 2>&1 | Out-String).Trim()
$clipA = Read-Clipboard
if ($Win10) {
    if ($logA -eq '') { Write-Pass "on this host nvim got no part of the drag, as measured" }
    else { Write-Fail "nvim logged [$logA], which this host was measured not to deliver" }
    if ($clipA -ne "CLIP_SENTINEL_575" -or $bufA -match "SELROW") { Write-Pass "and the drag was psmux's selection, as for any pane that asked for nothing" }
    else { Write-Fail "the drag was neither nvim's nor psmux's: clip='$clipA' buf='$bufA'" }
} else {
    if ($logA -match "press" -and $logA -match "drag") { Write-Pass "nvim received press+drag ($logA)" }
    else { Write-Fail "nvim missing gesture, log=[$logA]" }
    if ($clipA -eq "CLIP_SENTINEL_575" -and $bufA -notmatch "SELROW") { Write-Pass "psmux did not copy (clipboard sentinel intact)" }
    else { Write-Fail "psmux stole the gesture: clip='$clipA' buf='$bufA'" }
}

# --- Case B: force ON: psmux owns the drag, copies text, nvim sees no drag ---
Write-Host "[Case B] force on: psmux owns the drag" -ForegroundColor Yellow
& $PSMUX set-option -g mouse-selection-force on -t $S 2>&1 | Out-Null
Start-Sleep -Seconds 1
Clear-MLog
& $DRAG $cpid drag 0 4 20 4 6 60 | Out-Null
Start-Sleep -Seconds 2
$logB = Read-MLog
$bufB = (& $PSMUX show-buffer -t $S 2>&1 | Out-String).Trim()
$clipB = Read-Clipboard
if ($logB -notmatch "drag") { Write-Pass "no drag reached nvim (log=[$logB])" }
else { Write-Fail "drag leaked to nvim: [$logB]" }
if ($bufB -match "SELROW05" -or $clipB -match "SELROW05") { Write-Pass "dragged text copied (SELROW05)" }
else { Write-Fail "no copy: buf='$bufB' clip='$clipB'" }

# --- Case C: force ON: plain click replayed to nvim ---
Write-Host "[Case C] force on: plain click replayed" -ForegroundColor Yellow
Clear-MLog
& "$env:LOCALAPPDATA\Temp\psmux_mouse_drag_injector.exe" $cpid drag 10 6 10 6 1 60 | Out-Null
Start-Sleep -Seconds 2
$logC = Read-MLog
if ($logC -match "press" -and $logC -match "release") { Write-Pass "click replayed as press+release ($logC)" }
elseif ($Win10 -and $logC -eq '') { Write-Pass "on this host the replayed click did not reach nvim, as measured" }
else { Write-Fail "click not replayed, log=[$logC]" }

# --- Case D: force ON: wheel still reaches nvim ---
Write-Host "[Case D] force on: wheel still forwarded" -ForegroundColor Yellow
Clear-MLog
& $WHEEL $cpid up 3 20 5 | Out-Null
Start-Sleep -Seconds 2
$logD = Read-MLog
if ($logD -match "wheelup") { Write-Pass "wheel reached nvim ($logD)" }
elseif ($Win10 -and $logD -eq '') { Write-Pass "on this host the wheel did not reach nvim, as measured" }
else { Write-Fail "wheel did not reach nvim, log=[$logD]" }

# --- Case E: force off again returns to yield behavior ---
Write-Host "[Case E] force back off: nvim owns drag again" -ForegroundColor Yellow
& $PSMUX set-option -g mouse-selection-force off -t $S 2>&1 | Out-Null
Start-Sleep -Seconds 1
Clear-MLog
& $DRAG $cpid drag 2 8 25 8 6 60 | Out-Null
Start-Sleep -Seconds 2
$logE = Read-MLog
if ($logE -match "drag") { Write-Pass "drag reaches nvim again after toggling off ($logE)" }
elseif ($Win10 -and $logE -eq '') { Write-Pass "on this host the drag after toggling off did not reach nvim either, as measured" }
else { Write-Fail "drag no longer reaches nvim after toggle off, log=[$logE]" }

& $PSMUX kill-session -t $S 2>&1 | Out-Null
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
Restore
Write-Host "`n=== Results: Passed=$($script:Pass) Failed=$($script:Fail) ===" -ForegroundColor Cyan
exit $script:Fail
