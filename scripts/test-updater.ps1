param(
    [string]$CurrentVersion = '0.4.0'
)

$ErrorActionPreference = 'Stop'
$previous = [Environment]::GetEnvironmentVariable('RALGRUM_UPDATE_TEST_CURRENT_VERSION', 'Process')
try {
    $env:RALGRUM_UPDATE_TEST_CURRENT_VERSION = $CurrentVersion
    Write-Host "Starting the debug app as v$CurrentVersion for updater UI testing."
    Write-Host 'The updater will fetch and verify the real GitHub release. Debug installation stays disabled.'
    & (Join-Path $PSScriptRoot 'run-gpui.ps1')
} finally {
    [Environment]::SetEnvironmentVariable('RALGRUM_UPDATE_TEST_CURRENT_VERSION', $previous, 'Process')
}
