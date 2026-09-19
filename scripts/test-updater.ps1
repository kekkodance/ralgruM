param(
    [string]$CurrentVersion = '0.4.0',
    [switch]$FullFlow
)

$ErrorActionPreference = 'Stop'
$running = @(Get-Process -Name 'ralgruM' -ErrorAction SilentlyContinue)
if ($running.Count -gt 0) {
    throw 'Quit ralgruM from the system tray before starting the updater test. Closing its window may only hide it.'
}
$previous = [Environment]::GetEnvironmentVariable('RALGRUM_UPDATE_TEST_CURRENT_VERSION', 'Process')
$previousAsset = [Environment]::GetEnvironmentVariable('RALGRUM_UPDATE_TEST_ASSET', 'Process')
try {
    $env:RALGRUM_UPDATE_TEST_CURRENT_VERSION = $CurrentVersion
    if ($FullFlow) {
        $projectRoot = Split-Path -Parent $PSScriptRoot
        & cargo build --locked --manifest-path (Join-Path $projectRoot 'Cargo.toml')
        if ($LASTEXITCODE -ne 0) { throw 'The updater test build failed.' }
        $source = Join-Path $projectRoot 'target\debug\ralgruM.exe'
        $directory = Join-Path ([System.IO.Path]::GetTempPath()) "ralgrum-updater-test-$([guid]::NewGuid())"
        New-Item -ItemType Directory -Path $directory -ErrorAction Stop | Out-Null
        $target = Join-Path $directory 'ralgruM.exe'
        $candidate = Join-Path $directory 'candidate.exe'
        Copy-Item -LiteralPath $source -Destination $target -ErrorAction Stop
        Copy-Item -LiteralPath $source -Destination $candidate -ErrorAction Stop
        $marker = [System.Text.Encoding]::ASCII.GetBytes('ralgrum-updater-test-candidate')
        $stream = [System.IO.File]::Open($candidate, [System.IO.FileMode]::Append)
        try { $stream.Write($marker, 0, $marker.Length) } finally { $stream.Dispose() }
        $env:RALGRUM_UPDATE_TEST_ASSET = $candidate
        Write-Host "Updater full-flow fixture: $directory"
        Write-Host 'The candidate has a distinct hash. After Restart to update, the same ralgruM.exe path should relaunch with a new PID and match candidate.exe.'
        $process = Start-Process -FilePath $target -WorkingDirectory $projectRoot -WindowStyle Normal -PassThru
        Write-Host "Initial test PID: $($process.Id)"
        Write-Host "Original SHA-256: $((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash)"
        Write-Host "Candidate SHA-256: $((Get-FileHash -LiteralPath $candidate -Algorithm SHA256).Hash)"
        return
    }
    Write-Host "Starting the debug app as v$CurrentVersion for updater UI testing."
    Write-Host 'The updater will fetch and verify the real GitHub release. Debug installation stays disabled.'
    & (Join-Path $PSScriptRoot 'run-gpui.ps1')
} finally {
    [Environment]::SetEnvironmentVariable('RALGRUM_UPDATE_TEST_CURRENT_VERSION', $previous, 'Process')
    [Environment]::SetEnvironmentVariable('RALGRUM_UPDATE_TEST_ASSET', $previousAsset, 'Process')
}
