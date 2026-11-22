# Installs the native build's installer on a GitHub runner the way the
# updater and a person would, and checks what it leaves: files, shortcuts,
# the uninstall entry, one process and no WebView2, the idle cost, the
# single-instance rule, an upgrade over the last Tauri build, and a clean
# uninstall that keeps the user's data. rust.yml runs it after the build.
#
# Meant for a throwaway Windows machine: it installs over any copy there and
# writes the HKCU Run key, so do not run it on your own desktop.
#
#   pwsh -File tools/native-install-check.ps1 -Installer <setup.exe> -Version 1.1.1 `
#        -Exe src-tauri\target\release\aevaleron.exe -Out out [-PreviousBranch rust/next]
#
# Prints one "[CHECK] ok|FAIL name detail" line per check and
# "[METRIC] name=value" lines (tools/ci-publish.mjs puts those in the
# summary table). Exit 1 when any check failed. The upgrade needs gh and
# GH_TOKEN with actions: read; without them it prints
# "[METRIC] upgrade=skipped:<why>" and goes on.

param(
  [Parameter(Mandatory)] [string] $Installer,
  [Parameter(Mandatory)] [string] $Version,
  [Parameter(Mandatory)] [string] $Exe,
  [Parameter(Mandatory)] [string] $Out,
  [string] $PreviousBranch = 'rust/next'
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
New-Item -ItemType Directory -Force $Out | Out-Null
$Out = (Resolve-Path $Out).Path
$Installer = (Resolve-Path $Installer).Path
$Exe = (Resolve-Path $Exe).Path
$onRunner = [bool]$env:GITHUB_ACTIONS
$scratch = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }

$InstallDir = Join-Path $env:LOCALAPPDATA 'Aevaleron'
$AppExe = Join-Path $InstallDir 'Aevaleron.exe'
$Uninstaller = Join-Path $InstallDir 'uninstall.exe'
$DesktopDir = Join-Path $InstallDir 'desktop'
$WebViewDir = Join-Path $env:LOCALAPPDATA 'aevaleron\WebView2'
$UserData = Join-Path $env:APPDATA 'aevaleron'
$UninstKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Aevaleron'
$RunKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$RunValue = 'com.aevaleron.app'
$Aumid = 'com.aevaleron.app'
$StartMenuLnk = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Aevaleron.lnk'
$DesktopLnk = Join-Path ([Environment]::GetFolderPath('Desktop')) 'Aevaleron.lnk'

$checks = [System.Collections.Generic.List[object]]::new()
function Check([string]$name, [bool]$ok, [string]$detail = '') {
  $checks.Add([pscustomobject]@{ Name = $name; Ok = $ok; Detail = $detail })
  $mark = if ($ok) { 'ok' } else { 'FAIL' }
  Write-Host "[CHECK] $mark $name $detail"
}

$metrics = [ordered]@{}
function Metric([string]$name, $value) {
  $metrics[$name] = "$value"
  Write-Host "[METRIC] $name=$value"
}

function Annotate([string]$level, [string]$title, [string]$message) {
  if (-not $onRunner) { return }
  $m = $message.Replace('%', '%25').Replace("`r", '%0D').Replace("`n", '%0A')
  $t = $title.Replace('%', '%25').Replace(':', '%3A').Replace(',', '%2C')
  Write-Host "::${level} title=${t}::$m"
}

# An exception still ends with a readable annotation and the checks so far.
trap {
  $at = $_.InvocationInfo
  Annotate 'error' 'native-install-check.ps1' "line $($at.ScriptLineNumber): $($_.Exception.Message)`n$($at.Line.Trim())"
  Check 'script' $false "line $($at.ScriptLineNumber): $($_.Exception.Message)"
  break
}

function Entries {
  @(Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*' -ErrorAction SilentlyContinue |
      Where-Object { $_.DisplayName -like 'Aevaleron*' })
}

function App-Processes { @(Get-Process -Name Aevaleron -ErrorAction SilentlyContinue) }

function Stop-App {
  App-Processes | Stop-Process -Force -ErrorAction SilentlyContinue
  $deadline = (Get-Date).AddSeconds(10)
  while ((App-Processes).Count -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 250 }
}

# A setup.exe, silently, waiting for it; returns the exit code.
function Run-Setup([string]$file, [string]$label) {
  $p = Start-Process -FilePath $file -ArgumentList '/S' -PassThru
  $null = $p.Handle
  if (-not $p.WaitForExit(180000)) {
    Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
    return 'timeout'
  }
  Write-Host "$label exited $($p.ExitCode)"
  $p.ExitCode
}

# The app and every process under it.
function Tree([int]$root) {
  $all = @(Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, Name)
  $ids = [System.Collections.Generic.List[int]]::new()
  $ids.Add($root)
  for ($i = 0; $i -lt $ids.Count; $i++) {
    foreach ($c in $all | Where-Object { $_.ParentProcessId -eq $ids[$i] -and $_.ProcessId -ne $ids[$i] }) {
      if (-not $ids.Contains([int]$c.ProcessId)) { $ids.Add([int]$c.ProcessId) }
    }
  }
  $all | Where-Object { $ids.Contains([int]$_.ProcessId) }
}

function Lnk-Aumid([string]$path) {
  try {
    $shell = New-Object -ComObject Shell.Application
    $item = $shell.Namespace((Split-Path $path)).ParseName((Split-Path $path -Leaf))
    "$($item.ExtendedProperty('System.AppUserModel.ID'))"
  } catch { "error: $($_.Exception.Message)" }
}

function Check-Installed([string]$phase) {
  Check "$phase.exe" (Test-Path $AppExe) $AppExe
  $same = (Test-Path $AppExe) -and ((Get-FileHash $AppExe).Hash -eq (Get-FileHash $Exe).Hash)
  Check "$phase.exeIsThisBuild" $same 'installed Aevaleron.exe is the built aevaleron.exe'
  $fileVersion = if (Test-Path $AppExe) { (Get-Item $AppExe).VersionInfo.ProductVersion } else { '' }
  # Windows keeps four parts, so 1.1.1 can read as 1.1.1.0.
  Check "$phase.exeVersion" ($fileVersion -in $Version, "$Version.0") "exe=$fileVersion want=$Version"
  # The helper runs in process: no desktop\ folder, even after an upgrade.
  Check "$phase.noHelperDll" (-not (Test-Path $DesktopDir)) "desktop\ present: $(Test-Path $DesktopDir)"
  Check "$phase.uninstaller" (Test-Path $Uninstaller) $Uninstaller
  Check "$phase.startMenuShortcut" (Test-Path $StartMenuLnk) $StartMenuLnk
  Check "$phase.desktopShortcut" (Test-Path $DesktopLnk) $DesktopLnk
  if (Test-Path $StartMenuLnk) {
    $id = Lnk-Aumid $StartMenuLnk
    Check "$phase.shortcutAumid" ($id -eq $Aumid) "aumid=$id"
  }
  $entries = Entries
  Check "$phase.oneUninstallEntry" ($entries.Count -eq 1) "entries=$($entries.Count): $($entries.PSChildName -join ', ')"
  $key = Get-ItemProperty $UninstKey -ErrorAction SilentlyContinue
  Check "$phase.uninstallKey" ([bool]$key) $UninstKey
  if ($key) {
    Check "$phase.displayVersion" ($key.DisplayVersion -eq $Version) "registered=$($key.DisplayVersion)"
    Check "$phase.publisher" ($key.Publisher -eq 'darkelights') "publisher=$($key.Publisher)"
    $want = @('DisplayName', 'DisplayIcon', 'InstallLocation', 'UninstallString', 'QuietUninstallString', 'NoModify', 'NoRepair', 'EstimatedSize')
    $missing = @($want | Where-Object { $null -eq $key.$_ -or "$($key.$_)" -eq '' })
    Check "$phase.uninstallFields" ($missing.Count -eq 0) "missing=$($missing -join ',')"
  }
  Check "$phase.noWebView2Data" (-not (Test-Path $WebViewDir)) $WebViewDir
  $running = App-Processes
  Check "$phase.notLaunched" ($running.Count -eq 0) "processes=$($running.Count)"
  Stop-App
}

Metric 'installerKB' ([Math]::Round((Get-Item $Installer).Length / 1KB))

# ------------------------------------------------------------------ upgrade

# The last green Tauri build on $PreviousBranch, then this one over it.
$previous = $null
$why = ''
if (-not (Get-Command gh -ErrorAction SilentlyContinue)) { $why = 'no gh' }
elseif (-not $env:GH_TOKEN) { $why = 'no GH_TOKEN' }
else {
  try {
    $repo = if ($env:GITHUB_REPOSITORY) { @('--repo', $env:GITHUB_REPOSITORY) } else { @() }
    $id = gh run list @repo --workflow rust.yml --branch $PreviousBranch --status success --limit 1 --json databaseId -q '.[0].databaseId' 2>&1
    if ($LASTEXITCODE -ne 0 -or "$id" -notmatch '^\d+$') { $why = "no run: $id" }
    else {
      $dl = Join-Path $scratch 'previous-installer'
      Remove-Item -Recurse -Force $dl -ErrorAction SilentlyContinue
      $got = gh run download "$id" @repo -n aevaleron-tauri-nsis -D $dl 2>&1
      $previous = Get-ChildItem $dl -Filter '*.exe' -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1
      if (-not $previous) { $why = "download of run $id failed: $got" }
      else { Write-Host "previous installer: $($previous.Name) from run $id" }
    }
  } catch { $why = $_.Exception.Message }
}

if ($previous) {
  $code = Run-Setup $previous.FullName 'previous setup'
  Check 'previous.install' ($code -eq 0) "code=$code $($previous.Name)"
  Stop-App
  # The Tauri build makes its WebView2 profile on first start; make sure one
  # is there for the upgrade to remove.
  New-Item -ItemType Directory -Force $WebViewDir | Out-Null
  Set-Content (Join-Path $WebViewDir 'Local State') '{}'
  $before = (Get-ItemProperty $UninstKey -ErrorAction SilentlyContinue).DisplayVersion
  $code = Run-Setup $Installer 'setup over previous'
  Check 'upgrade.exitCode' ($code -eq 0) "code=$code from=$before"
  Check-Installed 'upgrade'
  Metric 'upgrade' $(if (@($checks | Where-Object { $_.Name -like 'upgrade.*' -and -not $_.Ok }).Count) { 'fail' } else { "ok:from-$before" })
} else {
  $why = ($why -replace '\s+', ' ').Trim()
  if ($why.Length -gt 120) { $why = $why.Substring(0, 120) }
  Metric 'upgrade' "skipped:$why"
  $code = Run-Setup $Installer 'setup'
  Check 'install.exitCode' ($code -eq 0) "code=$code"
  Check-Installed 'install'
}

# ------------------------------------------------------------------ run it

$appProfile = Join-Path $scratch 'aeval-native-install'
New-Item -ItemType Directory -Force $appProfile | Out-Null
$env:AEVAL_USERDATA = $appProfile
$webviewBefore = @(Get-Process -Name msedgewebview2 -ErrorAction SilentlyContinue).Count
$app = Start-Process -FilePath $AppExe -PassThru -RedirectStandardOutput (Join-Path $Out 'app.out.log') -RedirectStandardError (Join-Path $Out 'app.err.log')
$null = $app.Handle
Start-Sleep -Seconds 15
$alive = -not $app.HasExited
Check 'run.started' $alive "pid=$($app.Id) exited=$($app.HasExited)"

$apps = App-Processes
Check 'run.oneProcess' ($apps.Count -eq 1) "Aevaleron.exe processes=$($apps.Count)"
$webviewAfter = @(Get-Process -Name msedgewebview2 -ErrorAction SilentlyContinue).Count
$tree = @(if ($alive) { Tree $app.Id })
$webviewInTree = @($tree | Where-Object { $_.Name -like 'msedgewebview2*' }).Count
Check 'run.noWebView2' ($webviewInTree -eq 0 -and $webviewAfter -le $webviewBefore) "inTree=$webviewInTree before=$webviewBefore after=$webviewAfter"
Metric 'processCount' $tree.Count
if ($tree.Count -gt 1) { Write-Host "process tree: $(($tree | ForEach-Object { "$($_.Name)#$($_.ProcessId)" }) -join ' ')" }

if ($alive) {
  $p = Get-Process -Id $app.Id
  Metric 'idlePrivateMB' ([Math]::Round($p.PrivateMemorySize64 / 1MB, 1))
  Metric 'idleWorkingSetMB' ([Math]::Round($p.WorkingSet64 / 1MB, 1))
  $cpu0 = $p.TotalProcessorTime
  $t0 = Get-Date
  Start-Sleep -Seconds 10
  $p.Refresh()
  $spent = ($p.TotalProcessorTime - $cpu0).TotalMilliseconds
  $wall = ((Get-Date) - $t0).TotalMilliseconds
  # Of the whole machine, like Task Manager's CPU column.
  $pct = 100 * $spent / ($wall * [Environment]::ProcessorCount)
  Metric 'idleCpuPct' ([Math]::Round($pct, 2))
  Metric 'idleThreads' $p.Threads.Count
} else {
  Metric 'idlePrivateMB' 'n/a'
  Metric 'idleCpuPct' 'n/a'
}

# LIFE-01: a second launch hands over to the first and exits.
$t = Get-Date
$second = Start-Process -FilePath $AppExe -PassThru
$null = $second.Handle
$quit = $second.WaitForExit(10000)
$ms = [int]((Get-Date) - $t).TotalMilliseconds
if (-not $quit) { Stop-Process -Id $second.Id -Force -ErrorAction SilentlyContinue }
Check 'singleInstance.secondExits' $quit "exited=$quit after ${ms}ms"
Start-Sleep -Seconds 1
$count = (App-Processes).Count
Check 'singleInstance.oneLeft' ($count -eq 1) "processes=$count"
Metric 'secondLaunchMs' $ms

Stop-App
Remove-Item Env:\AEVAL_USERDATA

# ------------------------------------------------------------------ uninstall

# Things the uninstaller must remove, and the user data it must keep.
New-Item -ItemType Directory -Force $UserData | Out-Null
$keep = Join-Path $UserData 'ci-keep.txt'
Set-Content $keep 'kept'
New-ItemProperty -Path $RunKey -Name $RunValue -Value "`"$AppExe`"" -PropertyType String -Force | Out-Null

$key = Get-ItemProperty $UninstKey -ErrorAction SilentlyContinue
$cmd = if ($key.QuietUninstallString) { $key.QuietUninstallString } else { "`"$Uninstaller`" /S" }
$parts = [regex]::Match($cmd, '^"([^"]+)"\s*(.*)$')
Start-Process -FilePath $parts.Groups[1].Value -ArgumentList $parts.Groups[2].Value -Wait
# The NSIS uninstaller copies itself to %TEMP% and returns at once, so the
# files going away is the only sign it finished.
$deadline = (Get-Date).AddSeconds(90)
while (((Test-Path $AppExe) -or (Test-Path $Uninstaller)) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 500 }
Start-Sleep -Seconds 2
Check 'uninstall.exe' (-not (Test-Path $AppExe)) $AppExe
Check 'uninstall.uninstaller' (-not (Test-Path $Uninstaller)) $Uninstaller
Check 'uninstall.desktopHelper' (-not (Test-Path $DesktopDir)) $DesktopDir
Check 'uninstall.startMenuShortcut' (-not (Test-Path $StartMenuLnk)) ''
Check 'uninstall.desktopShortcut' (-not (Test-Path $DesktopLnk)) ''
Check 'uninstall.key' (-not (Test-Path $UninstKey)) $UninstKey
Check 'uninstall.noEntries' ((Entries).Count -eq 0) "entries=$((Entries).Count)"
$run = (Get-ItemProperty $RunKey -ErrorAction SilentlyContinue).$RunValue
Check 'uninstall.runValue' (-not $run) "Run\$RunValue=$run"
Check 'uninstall.userDataKept' (Test-Path $keep) $keep

# ------------------------------------------------------------------ report

$failed = @($checks | Where-Object { -not $_.Ok })
$rows = $checks | ForEach-Object { "| $(if ($_.Ok) { 'ok' } else { '**FAIL**' }) | $($_.Name) | $($_.Detail.Replace('|', '\|')) |" }
$mrows = $metrics.Keys | ForEach-Object { "| $_ | $($metrics[$_]) |" }
$md = @(
  "## Native install check: Aevaleron $Version"
  ''
  "$($checks.Count - $failed.Count) of $($checks.Count) passed."
  ''
  '| Metric | Value |'
  '| --- | --- |'
) + $mrows + @(
  ''
  '| | Check | Detail |'
  '| --- | --- | --- |'
) + $rows
$md -join "`n" | Set-Content (Join-Path $Out 'summary.md') -Encoding utf8
if ($env:GITHUB_STEP_SUMMARY) { $md -join "`n" | Add-Content $env:GITHUB_STEP_SUMMARY -Encoding utf8 }

foreach ($f in $failed) { Annotate 'error' $f.Name $f.Detail }
$mline = ($metrics.Keys | ForEach-Object { "$_=$($metrics[$_])" }) -join ' '
Annotate 'notice' "native install: $($checks.Count - $failed.Count) of $($checks.Count) passed" $mline

if ($failed.Count) { exit 1 }
exit 0
