$ErrorActionPreference = 'Stop'

$sourceRoot = Join-Path $PSScriptRoot '..\src'
$allowedSourceContract = [System.IO.Path]::GetFullPath(
    (Join-Path $sourceRoot 'murglar_backend\privacy_tests.rs')
)
$violations = Get-ChildItem -LiteralPath $sourceRoot -Recurse -File -Filter '*.rs' |
    Where-Object { $_.FullName -ne $allowedSourceContract } |
    Select-String -Pattern 'include_str!\("[^"]+\.rs"\)'

if ($violations) {
    $violations | ForEach-Object {
        Write-Error "Source-text test found at $($_.Path):$($_.LineNumber). Test behavior through callable code instead."
    }
    exit 1
}

Write-Output 'Test-quality check passed.'
