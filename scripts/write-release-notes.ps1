param(
    [Parameter(Mandatory = $true)]
    [string]$Version,

    [Parameter(Mandatory = $true)]
    [string]$OutputPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repository = 'kekkodance/ralgruM'
$previousTag = (& git describe --tags --abbrev=0 --match 'v[0-9]*' HEAD 2>$null)
if ($LASTEXITCODE -ne 0) {
    $previousTag = $null
}

$range = if ($previousTag) { "$previousTag..HEAD" } else { 'HEAD' }
$commits = @(& git log --format='%H%x09%s' $range)
if ($LASTEXITCODE -ne 0) {
    throw "Could not collect commits for release notes from '$range'."
}

$lines = [System.Collections.Generic.List[string]]::new()
$lines.Add("## Changelog for v$Version")
$lines.Add('')
if ($previousTag) {
    $compareUrl = "https://github.com/$repository/compare/$previousTag...v$Version"
    $lines.Add("Commits since [$previousTag]($compareUrl):")
} else {
    $lines.Add('Commits in this release:')
}
$lines.Add('')

if ($commits.Count -eq 0) {
    $lines.Add('No source commits since the previous release.')
} else {
    foreach ($commit in $commits) {
        $parts = $commit -split "`t", 2
        if ($parts.Count -ne 2 -or $parts[0] -notmatch '^[0-9a-f]{40}$') {
            throw "Unexpected git log entry: '$commit'."
        }
        $sha = $parts[0]
        $shortSha = $sha.Substring(0, 7)
        $subject = $parts[1].Replace('&', '&amp;').Replace('<', '&lt;').Replace('>', '&gt;')
        $lines.Add("- [$shortSha](https://github.com/$repository/commit/$sha) $subject")
    }
}

$directory = Split-Path -Parent $OutputPath
if ($directory -and -not (Test-Path -LiteralPath $directory)) {
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
}
$lines | Set-Content -LiteralPath $OutputPath -Encoding utf8
Write-Host "Wrote $($commits.Count) commits to $OutputPath"
