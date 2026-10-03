$ErrorActionPreference = 'Stop'

# Path separators are normalized to forward slashes and Join-Path results are
# canonicalized, so the same script runs under Windows PowerShell 5.1 and
# pwsh on Linux runners. Plain Join-Path keeps the scripts\..\ segment, which
# never matches a FullName, and GetFullPath collapses it on both platforms.
$sourceRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..' | Join-Path -ChildPath 'src'))
$allowedSourceContract = [System.IO.Path]::GetFullPath(
    (Join-Path $sourceRoot 'murglar_backend' | Join-Path -ChildPath 'privacy_tests.rs')
).Replace('\', '/')
$violations = Get-ChildItem -LiteralPath $sourceRoot -Recurse -File -Filter '*.rs' |
    Where-Object { $_.FullName.Replace('\', '/') -ne $allowedSourceContract } |
    Select-String -Pattern 'include_str!\("[^"]+\.rs"\)'

if ($violations) {
    $violations | ForEach-Object {
        Write-Error "Source-text test found at $($_.Path):$($_.LineNumber). Test behavior through callable code instead."
    }
    exit 1
}

Write-Output 'Test-quality check passed.'
