param(
    [switch]$Release,
    [switch]$BuildOnly
)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Set-Location $projectRoot

$maxCaptureFiles = 40
$maxCaptureBytes = 16MB
$maxLauncherLogBytes = 1MB

function Remove-OldGpuiCaptureLogs {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Directory,
        [Parameter(Mandatory = $true)]
        [int]$MaximumFiles,
        [Parameter(Mandatory = $true)]
        [long]$MaximumBytes,
        [string[]]$ExcludePaths = @()
    )

    if ($MaximumFiles -lt 1 -or $MaximumBytes -lt 1 -or -not (Test-Path -LiteralPath $Directory -PathType Container)) {
        return
    }

    $capturePattern = '^gpui-\d{8}T\d{9}Z\.(stdout|stderr)\.log$'
    $captures = @(
        Get-ChildItem -LiteralPath $Directory -File -Force |
            Where-Object { $_.Name -cmatch $capturePattern } |
            Sort-Object Name -Descending
    )
    $excluded = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::OrdinalIgnoreCase
    )
    foreach ($path in $ExcludePaths) {
        if (-not [string]::IsNullOrWhiteSpace($path)) {
            [void]$excluded.Add([System.IO.Path]::GetFullPath($path))
        }
    }

    $preserved = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::OrdinalIgnoreCase
    )
    $newestCount = [Math]::Min(2, $captures.Count)
    foreach ($capture in ($captures | Select-Object -First $newestCount)) {
        [void]$preserved.Add($capture.FullName)
    }
    foreach ($capture in $captures) {
        if ($excluded.Contains($capture.FullName)) {
            [void]$preserved.Add($capture.FullName)
        }
    }

    $oldFileLimit = [Math]::Max(0, $MaximumFiles - $preserved.Count)
    $retainedOldFiles = 0
    [long]$retainedOldBytes = 0
    foreach ($capture in $captures) {
        if ($preserved.Contains($capture.FullName)) {
            continue
        }

        try {
            $currentItem = Get-Item -LiteralPath $capture.FullName -Force -ErrorAction Stop
            [long]$captureBytes = $currentItem.Length
        } catch {
            if ($_.Exception -is [System.Management.Automation.ItemNotFoundException] -or
                $_.Exception -is [System.IO.FileNotFoundException] -or
                $_.Exception -is [System.IO.DirectoryNotFoundException]) {
                continue
            }
            Write-Warning "Could not inspect GPUI capture log $($capture.FullName): $($_.Exception.Message)"
            continue
        }
        $fitsByteBudget = $captureBytes -le ($MaximumBytes - $retainedOldBytes)
        if ($retainedOldFiles -lt $oldFileLimit -and $fitsByteBudget) {
            $retainedOldFiles++
            $retainedOldBytes += $captureBytes
            continue
        }

        try {
            Remove-Item -LiteralPath $capture.FullName -Force -ErrorAction Stop
        } catch {
            if ($_.Exception -is [System.Management.Automation.ItemNotFoundException] -or
                $_.Exception -is [System.IO.FileNotFoundException] -or
                $_.Exception -is [System.IO.DirectoryNotFoundException]) {
                continue
            }
            Write-Warning "Could not prune GPUI capture log $($capture.FullName): $($_.Exception.Message)"
        }
    }
}

function ConvertTo-PowerShellSingleQuotedLiteral {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Value
    )

    "'" + $Value.Replace("'", "''") + "'"
}

function Limit-GpuiLogFile {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,
        [Parameter(Mandatory = $true)]
        [long]$MaximumBytes
    )

    if ($MaximumBytes -lt 1 -or -not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        return
    }

    $item = Get-Item -LiteralPath $Path -Force
    if ($item.Length -le $MaximumBytes) {
        return
    }

    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $start = [Math]::Max(0, $bytes.Length - [int]$MaximumBytes)
    $newline = -1
    for ($index = $start; $index -lt $bytes.Length; $index++) {
        if ($bytes[$index] -eq 10) {
            $newline = $index
            break
        }
    }
    $keepStart = if ($newline -ge 0) { $newline + 1 } else { $start }
    $retained = if ($keepStart -lt $bytes.Length) {
        $bytes[$keepStart..($bytes.Length - 1)]
    } else {
        [byte[]]::new(0)
    }
    [System.IO.File]::WriteAllBytes($Path, $retained)
}

if (-not $BuildOnly) {
    $profileDirectory = if ($Release) { 'release' } else { 'debug' }
    $targetExe = [System.IO.Path]::GetFullPath(
        (Join-Path $projectRoot "target\$profileDirectory\ralgruM.exe")
    )
    $running = @(
        Get-Process -Name 'ralgruM','ralgrum-gpui' -ErrorAction SilentlyContinue |
            Where-Object {
                try {
                    [string]::Equals(
                        [System.IO.Path]::GetFullPath($_.Path),
                        $targetExe,
                        [System.StringComparison]::OrdinalIgnoreCase
                    )
                } catch {
                    $false
                }
            }
    )
    foreach ($process in $running) {
        if (-not $process.CloseMainWindow()) {
            throw "The project app (PID $($process.Id)) has no closable window. Close it manually before rebuilding."
        }
        if (-not $process.WaitForExit(5000)) {
            throw "The project app (PID $($process.Id)) did not close in time. Close it manually before rebuilding."
        }
    }
}

$timer = [System.Diagnostics.Stopwatch]::StartNew()
$buildArguments = @('build', '--locked', '--manifest-path', 'Cargo.toml')
if ($BuildOnly) {
    $buildArguments += @('--target-dir', 'target\build-only')
}
if ($Release) {
    $buildArguments += '--release'
}
& cargo @buildArguments
if ($LASTEXITCODE -ne 0) { throw 'Native GPUI build failed.' }
$timer.Stop()

$targetRoot = if ($BuildOnly) {
    'target\build-only'
} else {
    'target'
}
$profileDirectory = if ($Release) { 'release' } else { 'debug' }
$exePath = (Resolve-Path (Join-Path $targetRoot "$profileDirectory\ralgruM.exe")).Path
if ($BuildOnly) {
    Write-Host "ralgruM GPUI rebuilt in $([math]::Round($timer.Elapsed.TotalSeconds, 1))s."
    Write-Host "Executable: $exePath"
    return
}
$env:RUST_BACKTRACE = '1'
$logDirectory = if ($env:LOCALAPPDATA) {
    Join-Path $env:LOCALAPPDATA 'ralgruM\logs'
} else {
    $env:TEMP
}
New-Item -ItemType Directory -Path $logDirectory -Force | Out-Null
$runId = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ')
$captureStdio = [bool]$Release
$stdoutLog = $null
$stderrLog = $null
$launcherLog = Join-Path $logDirectory 'gpui-launcher.log'
$startTime = [DateTime]::UtcNow.ToString('o')

if ($captureStdio) {
    $stdoutLog = Join-Path $logDirectory "gpui-$runId.stdout.log"
    $stderrLog = Join-Path $logDirectory "gpui-$runId.stderr.log"
    $gpuiProcess = Start-Process -FilePath $exePath `
        -WorkingDirectory $projectRoot `
        -RedirectStandardOutput $stdoutLog `
        -RedirectStandardError $stderrLog `
        -WindowStyle Normal `
        -PassThru
} else {
    $gpuiProcess = Start-Process -FilePath $exePath `
        -WorkingDirectory $projectRoot `
        -WindowStyle Normal `
        -PassThru
}
$captureExcludePaths = @()
if ($stdoutLog) {
    $captureExcludePaths += $stdoutLog
}
if ($stderrLog) {
    $captureExcludePaths += $stderrLog
}
try {
    Remove-OldGpuiCaptureLogs `
        -Directory $logDirectory `
        -MaximumFiles $maxCaptureFiles `
        -MaximumBytes $maxCaptureBytes `
        -ExcludePaths $captureExcludePaths
} catch {
    Write-Warning "Could not prune GPUI capture logs: $($_.Exception.Message)"
}
if ($gpuiProcess.HasExited) {
    $startupError = if ($stderrLog -and (Test-Path -LiteralPath $stderrLog)) {
        (Get-Content -LiteralPath $stderrLog -Raw).Trim()
    } else {
        ''
    }
    if (-not $startupError) {
        $gpuiLog = Join-Path $logDirectory 'gpui.log'
        if (Test-Path -LiteralPath $gpuiLog) {
            $startupError = (Get-Content -LiteralPath $gpuiLog -Raw).Trim()
        }
    }
    if ($startupError) {
        throw "GPUI exited before launch: $startupError"
    }
    throw 'GPUI exited before launch.'
}
$gpuiPid = $gpuiProcess.Id

try {
    Limit-GpuiLogFile -Path $launcherLog -MaximumBytes $maxLauncherLogBytes
    $captureSummary = if ($stdoutLog -and $stderrLog) {
        "stdout=$stdoutLog stderr=$stderrLog"
    } else {
        "stdout=console stderr=console"
    }
    Add-Content -LiteralPath $launcherLog -Value "$([DateTime]::UtcNow.ToString('o')) pid=$gpuiPid start=$startTime $captureSummary"
    Limit-GpuiLogFile -Path $launcherLog -MaximumBytes $maxLauncherLogBytes
} catch {
    Write-Warning "Could not update the GPUI launcher log: $($_.Exception.Message)"
}

$windowReady = $false
$windowDeadline = [DateTime]::UtcNow.AddSeconds(10)
while ([DateTime]::UtcNow -lt $windowDeadline) {
    $gpuiProcess = Get-Process -Id $gpuiPid -ErrorAction SilentlyContinue
    if ($null -eq $gpuiProcess) {
        throw "GPUI exited before creating its window."
    }
    $gpuiProcess.Refresh()
    if ($gpuiProcess.HasExited) {
        $startupError = if ($stderrLog -and (Test-Path -LiteralPath $stderrLog)) {
            (Get-Content -LiteralPath $stderrLog -Raw).Trim()
        } else {
            ''
        }
        if ($startupError) {
            throw "GPUI exited before creating its window: $startupError"
        }
        throw "GPUI exited before creating its window (exit code $($gpuiProcess.ExitCode))."
    }
    if ([int64]$gpuiProcess.MainWindowHandle -ne 0) {
        $firstHandle = [int64]$gpuiProcess.MainWindowHandle
        $firstTitle = $gpuiProcess.MainWindowTitle
        if (-not [string]::IsNullOrWhiteSpace($firstTitle)) {
            $stableWindow = $true
            $stableDeadline = [DateTime]::UtcNow.AddSeconds(2)
            while ([DateTime]::UtcNow -lt $stableDeadline) {
                Start-Sleep -Milliseconds 250
                $gpuiProcess = Get-Process -Id $gpuiPid -ErrorAction SilentlyContinue
                if ($null -eq $gpuiProcess) {
                    $stableWindow = $false
                    break
                }
                $gpuiProcess.Refresh()
                if (
                    $gpuiProcess.HasExited `
                    -or [int64]$gpuiProcess.MainWindowHandle -ne $firstHandle `
                    -or [int64]$gpuiProcess.MainWindowHandle -eq 0 `
                    -or [string]::IsNullOrWhiteSpace($gpuiProcess.MainWindowTitle) `
                    -or -not $gpuiProcess.Responding
                ) {
                    $stableWindow = $false
                    break
                }
            }
            if ($stableWindow -and -not $gpuiProcess.HasExited) {
                $windowReady = $true
                break
            }
        }
    }
    Start-Sleep -Milliseconds 250
}
if (-not $windowReady) {
    $gpuiProcess = Get-Process -Id $gpuiPid -ErrorAction SilentlyContinue
    $processStillRunning = $null -ne $gpuiProcess
    if ($processStillRunning) {
        $gpuiProcess.Refresh()
    }
    $observedHandle = if ($processStillRunning) { [int64]$gpuiProcess.MainWindowHandle } else { 0 }
    $observedTitle = if ($processStillRunning) { $gpuiProcess.MainWindowTitle } else { '' }
    $observedResponding = if ($processStillRunning -and -not $gpuiProcess.HasExited) { $gpuiProcess.Responding } else { $false }
    $startupError = if ($stderrLog -and (Test-Path -LiteralPath $stderrLog)) {
        (Get-Content -LiteralPath $stderrLog -Raw).Trim()
    } else {
        ''
    }
    try {
        if ($processStillRunning -and -not $gpuiProcess.HasExited) {
            Stop-Process -Id $gpuiPid -Force
        }
    } catch {
    }
    if ($startupError) {
        throw "GPUI did not create a stable visible window within 10 seconds (MainWindowHandle=$observedHandle, MainWindowTitle='$observedTitle', Responding=$observedResponding): $startupError"
    }
    throw "GPUI did not create a stable visible window within 10 seconds (MainWindowHandle=$observedHandle, MainWindowTitle='$observedTitle', Responding=$observedResponding)."
}

$gpuiProcess = Get-Process -Id $gpuiPid -ErrorAction Stop
$gpuiProcess.Refresh()
$launcherLogLiteral = ConvertTo-PowerShellSingleQuotedLiteral $launcherLog
$stdoutLogLiteral = if ($stdoutLog) {
    ConvertTo-PowerShellSingleQuotedLiteral $stdoutLog
} else {
    "''"
}
$stderrLogLiteral = if ($stderrLog) {
    ConvertTo-PowerShellSingleQuotedLiteral $stderrLog
} else {
    "''"
}

$watcher = @"
`$process = [System.Diagnostics.Process]::GetProcessById($gpuiPid)
`$process.WaitForExit()
`$exitTime = [DateTime]::UtcNow.ToString('o')
`$currentCapturePaths = @($stdoutLogLiteral, $stderrLogLiteral)
try {
    Add-Content -LiteralPath $launcherLogLiteral -Value "`$exitTime pid=$gpuiPid exit_code=`$(`$process.ExitCode) exit_time=`$exitTime"
    try {
        `$launcherBytes = [System.IO.File]::ReadAllBytes($launcherLogLiteral)
        if (`$launcherBytes.Length -gt $maxLauncherLogBytes) {
            `$launcherStart = `$launcherBytes.Length - $maxLauncherLogBytes
            `$launcherNewline = -1
            for (`$index = `$launcherStart; `$index -lt `$launcherBytes.Length; `$index++) {
                if (`$launcherBytes[`$index] -eq 10) {
                    `$launcherNewline = `$index
                    break
                }
            }
            `$launcherKeepStart = if (`$launcherNewline -ge 0) {
                `$launcherNewline + 1
            } else {
                `$launcherStart
            }
            `$launcherRetained = if (`$launcherKeepStart -lt `$launcherBytes.Length) {
                `$launcherBytes[`$launcherKeepStart..(`$launcherBytes.Length - 1)]
            } else {
                [byte[]]::new(0)
            }
            [System.IO.File]::WriteAllBytes($launcherLogLiteral, `$launcherRetained)
        }
    } catch {
    }
} catch {
}
`$process.Dispose()
"@
Start-Process -FilePath 'powershell.exe' `
    -ArgumentList @('-NoProfile', '-NonInteractive', '-WindowStyle', 'Hidden', '-Command', $watcher) `
    -WindowStyle Hidden | Out-Null

Write-Host "ralgruM GPUI rebuilt in $([math]::Round($timer.Elapsed.TotalSeconds, 1))s and started asynchronously."
Write-Host "GPUI PID: $gpuiPid"
Write-Host "GPUI window handle: $([int64]$gpuiProcess.MainWindowHandle)"
Write-Host "GPUI window title: $($gpuiProcess.MainWindowTitle)"
Write-Host "GPUI responding: $($gpuiProcess.Responding)"
if ($stdoutLog -and $stderrLog) {
    Write-Host "stdout: $stdoutLog"
    Write-Host "stderr: $stderrLog"
} else {
    Write-Host "stdout: console"
    Write-Host "stderr: console"
}
Write-Host "launcher: $launcherLog"
