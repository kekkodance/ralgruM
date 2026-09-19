param(
    [string]$CurrentVersion = '0.4.0'
)

$ErrorActionPreference = 'Stop'
$running = @(Get-Process -Name 'ralgruM' -ErrorAction SilentlyContinue)
if ($running.Count -gt 0) {
    throw 'Quit ralgruM from the system tray before starting the updater test. Closing its window may only hide it.'
}
$previous = [Environment]::GetEnvironmentVariable('RALGRUM_UPDATE_TEST_CURRENT_VERSION', 'Process')
try {
    $env:RALGRUM_UPDATE_TEST_CURRENT_VERSION = $CurrentVersion
    Write-Host "Starting the debug app as v$CurrentVersion for updater UI testing."
    Write-Host 'The updater will fetch and verify the real GitHub release. Debug installation stays disabled.'
    & (Join-Path $PSScriptRoot 'run-gpui.ps1')
} finally {
    [Environment]::SetEnvironmentVariable('RALGRUM_UPDATE_TEST_CURRENT_VERSION', $previous, 'Process')
}
