<#
.SYNOPSIS
    Bumps the package version, builds the release executable, and stages it under dist/.

.DESCRIPTION
    Reads the [package] version from Cargo.toml, computes the next version using
    strict Semantic Versioning 2.0.0 rules (see semver.org), writes it back to
    Cargo.toml and Cargo.lock, then runs a locked release build and copies the
    executable to dist/ as ralgruM.exe.

    Precedence follows semver section 11: the numeric core compares first, a
    version with a prerelease has lower precedence than the same core without
    one, and build metadata is ignored. The new version must have higher
    precedence than the current version unless -Force is given.

    -Bump always increments the requested core digit, resets lower digits to
    zero, and drops any prerelease and build metadata. Promoting a prerelease
    such as 1.2.3-alpha to its release uses an explicit -Version 1.2.3.

.PARAMETER Bump
    Core digit to increment: Major, Minor, or Patch. Cannot be combined
    with -Version.

.PARAMETER Prerelease
    Optional prerelease label appended to a -Bump result, for example
    "beta.1". Only valid together with -Bump.

.PARAMETER Build
    Optional build metadata appended to a -Bump result, for example "001".
    Only valid together with -Bump. Build metadata does not affect precedence
    and is ignored by Cargo dependency resolution.

.PARAMETER Version
    Exact version to release (full semver, an optional leading "v" is allowed).
    Cannot be combined with -Bump, -Prerelease, or -Build.

.PARAMETER Force
    Allow a new version whose precedence is not higher than the current version
    (same-version rebuild or downgrade).

.PARAMETER NoBuild
    Update the version files without running cargo build or staging dist/.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1
    Pick Patch, Minor, Major, or a custom version interactively.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1 -Bump Patch

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1 -Bump Minor -Prerelease beta.1

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1 -Version 1.2.3

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1 -Version v1.2.3-rc.1
#>
[CmdletBinding(SupportsShouldProcess = $true, DefaultParameterSetName = 'Interactive')]
param(
    [Parameter(ParameterSetName = 'Bump', Mandatory = $true)]
    [ValidateSet('Major', 'Minor', 'Patch')]
    [string]$Bump,

    [Parameter(ParameterSetName = 'Bump')]
    [string]$Prerelease,

    [Parameter(ParameterSetName = 'Bump')]
    [string]$Build,

    [Parameter(ParameterSetName = 'Exact', Mandatory = $true)]
    [string]$Version,

    [switch]$Force,

    [switch]$NoBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Numerics | Out-Null

$projectRoot = Split-Path -Parent $PSScriptRoot

$script:SemVerCorePattern = '(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)'
$script:SemVerPrereleasePattern = '(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9A-Za-z-]*))*))?'
$script:SemVerBuildPattern = '(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?'
$script:SemVerPattern = '^' + $script:SemVerCorePattern + $script:SemVerPrereleasePattern + $script:SemVerBuildPattern + '$'
$script:NumericIdentifierPattern = '^(0|[1-9]\d*)$'

function Normalize-Version {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyString()]
        [string]$Value
    )

    $trimmed = $Value.Trim()
    if ($trimmed.Length -gt 1 -and ($trimmed[0] -eq 'v' -or $trimmed[0] -eq 'V') -and [char]::IsDigit($trimmed[1])) {
        $trimmed = $trimmed.Substring(1)
    }

    return $trimmed
}

function Test-SemVer {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyString()]
        [string]$Value
    )

    return [regex]::IsMatch($Value, $script:SemVerPattern)
}

function ConvertFrom-SemVer {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Value
    )

    $match = [regex]::Match($Value, $script:SemVerPattern)
    if (-not $match.Success) {
        throw "Invalid version '$Value'. Expected semver X.Y.Z with optional -prerelease and +build (see semver.org)."
    }

    $prerelease = ''
    if ($match.Groups[4].Success) {
        $prerelease = $match.Groups[4].Value
    }
    $build = ''
    if ($match.Groups[5].Success) {
        $build = $match.Groups[5].Value
    }

    [pscustomobject]@{
        Major      = $match.Groups[1].Value
        Minor      = $match.Groups[2].Value
        Patch      = $match.Groups[3].Value
        Prerelease = $prerelease
        Build      = $build
    }
}

function Compare-NumericIdentifier {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Left,
        [Parameter(Mandatory = $true)]
        [string]$Right
    )

    # Inputs are validated semver (no leading zeros), so a longer digit string
    # is the larger number. This avoids integer overflow entirely.
    if ($Left.Length -ne $Right.Length) {
        if ($Left.Length -lt $Right.Length) {
            return -1
        }
        return 1
    }

    $result = [string]::CompareOrdinal($Left, $Right)
    if ($result -lt 0) {
        return -1
    }
    if ($result -gt 0) {
        return 1
    }
    return 0
}

function Compare-SemVer {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Left,
        [Parameter(Mandatory = $true)]
        [string]$Right
    )

    # Returns -1 when Left < Right, 0 when equal precedence, 1 when Left > Right.
    # Build metadata is ignored per semver section 11.
    $a = ConvertFrom-SemVer -Value (Normalize-Version -Value $Left)
    $b = ConvertFrom-SemVer -Value (Normalize-Version -Value $Right)

    foreach ($field in @('Major', 'Minor', 'Patch')) {
        $compared = Compare-NumericIdentifier -Left $a.$field -Right $b.$field
        if ($compared -ne 0) {
            return $compared
        }
    }

    $aHasPre = -not [string]::IsNullOrEmpty($a.Prerelease)
    $bHasPre = -not [string]::IsNullOrEmpty($b.Prerelease)
    if (-not $aHasPre -and -not $bHasPre) {
        return 0
    }
    if (-not $aHasPre) {
        return 1
    }
    if (-not $bHasPre) {
        return -1
    }

    $aIds = $a.Prerelease.Split('.')
    $bIds = $b.Prerelease.Split('.')
    $count = [Math]::Max($aIds.Count, $bIds.Count)
    for ($i = 0; $i -lt $count; $i++) {
        if ($i -ge $aIds.Count) {
            return -1
        }
        if ($i -ge $bIds.Count) {
            return 1
        }
        $aId = $aIds[$i]
        $bId = $bIds[$i]
        $aNumeric = $aId -match $script:NumericIdentifierPattern
        $bNumeric = $bId -match $script:NumericIdentifierPattern
        if ($aNumeric -and $bNumeric) {
            $compared = Compare-NumericIdentifier -Left $aId -Right $bId
            if ($compared -ne 0) {
                return $compared
            }
        } elseif ($aNumeric) {
            return -1
        } elseif ($bNumeric) {
            return 1
        } else {
            $compared = [string]::CompareOrdinal($aId, $bId)
            if ($compared -lt 0) {
                return -1
            }
            if ($compared -gt 0) {
                return 1
            }
        }
    }

    return 0
}

function Get-BumpedVersion {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Value,
        [Parameter(Mandatory = $true)]
        [ValidateSet('Major', 'Minor', 'Patch')]
        [string]$Bump,
        [string]$Prerelease = '',
        [string]$Build = ''
    )

    $current = ConvertFrom-SemVer -Value (Normalize-Version -Value $Value)
    $major = [System.Numerics.BigInteger]::Parse($current.Major)
    $minor = [System.Numerics.BigInteger]::Parse($current.Minor)
    $patch = [System.Numerics.BigInteger]::Parse($current.Patch)
    $one = [System.Numerics.BigInteger]::One
    $zero = [System.Numerics.BigInteger]::Zero

    if ($Bump -eq 'Major') {
        $major = $major + $one
        $minor = $zero
        $patch = $zero
    } elseif ($Bump -eq 'Minor') {
        $minor = $minor + $one
        $patch = $zero
    } else {
        $patch = $patch + $one
    }

    $next = '{0}.{1}.{2}' -f $major.ToString(), $minor.ToString(), $patch.ToString()
    if (-not [string]::IsNullOrWhiteSpace($Prerelease)) {
        $next += '-' + $Prerelease.Trim()
    }
    if (-not [string]::IsNullOrWhiteSpace($Build)) {
        $next += '+' + $Build.Trim()
    }
    if (-not (Test-SemVer -Value $next)) {
        throw "Computed version '$next' is not valid semver."
    }

    return $next
}

function Read-Utf8File {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $utf8 = New-Object System.Text.UTF8Encoding $false
    [System.IO.File]::ReadAllText((Resolve-Path -LiteralPath $Path).Path, $utf8)
}

function Write-Utf8File {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,
        [Parameter(Mandatory = $true)]
        [string]$Content
    )

    $utf8 = New-Object System.Text.UTF8Encoding $false
    [System.IO.File]::WriteAllText($Path, $Content, $utf8)
}

function Get-FileNewline {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Content
    )

    if ($Content.Contains("`r`n")) {
        return "`r`n"
    }

    return "`n"
}

function Get-TextLines {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyString()]
        [string]$Content
    )

    # Regex.Split keeps trailing empty lines. Avoid ` -split , -1` because PowerShell 7
    # treats a negative max as split-from-the-end.
    return [regex]::Split($Content, '\r?\n')
}

function Get-TomlPackageVersionIndex {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyString()]
        [string[]]$Lines
    )

    $inPackage = $false
    for ($index = 0; $index -lt $Lines.Count; $index++) {
        $line = $Lines[$index]
        if ($line -match '^\s*\[package\]\s*(#.*)?$') {
            $inPackage = $true
            continue
        }
        if ($inPackage -and $line -match '^\s*\[') {
            break
        }
        if ($inPackage -and $line -match '^\s*version\s*=\s*"[^"]*"\s*(#.*)?$') {
            return $index
        }
    }

    return -1
}

function Get-TomlPackageName {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyString()]
        [string[]]$Lines
    )

    $inPackage = $false
    for ($index = 0; $index -lt $Lines.Count; $index++) {
        $line = $Lines[$index]
        if ($line -match '^\s*\[package\]\s*(#.*)?$') {
            $inPackage = $true
            continue
        }
        if ($inPackage -and $line -match '^\s*\[') {
            break
        }
        if ($inPackage -and $line -match '^\s*name\s*=\s*"([^"]+)"\s*(#.*)?$') {
            return $Matches[1]
        }
    }

    return ''
}

function Get-LockfilePackageVersionIndex {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyString()]
        [string[]]$Lines,
        [Parameter(Mandatory = $true)]
        [string]$PackageName
    )

    $escapedName = [regex]::Escape($PackageName)
    $inPackage = $false
    $nameMatched = $false
    for ($index = 0; $index -lt $Lines.Count; $index++) {
        $line = $Lines[$index]
        if ($line -match '^\s*\[\[package\]\]\s*$') {
            $inPackage = $true
            $nameMatched = $false
            continue
        }
        if ($inPackage -and $line -match '^\s*\[') {
            $inPackage = $false
            $nameMatched = $false
            continue
        }
        if ($inPackage -and $line -match ('^\s*name\s*=\s*"' + $escapedName + '"\s*$')) {
            $nameMatched = $true
            continue
        }
        if ($inPackage -and $nameMatched -and $line -match '^\s*version\s*=\s*"') {
            return $index
        }
    }

    return -1
}

function Get-CargoPackageName {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $content = Read-Utf8File -Path $Path
    $name = Get-TomlPackageName -Lines (Get-TextLines -Content $content)
    if ([string]::IsNullOrWhiteSpace($name)) {
        throw "Could not find [package] name in $Path."
    }

    return $name
}

function Get-CargoPackageVersion {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $content = Read-Utf8File -Path $Path
    $lines = Get-TextLines -Content $content
    $index = Get-TomlPackageVersionIndex -Lines $lines
    if ($index -lt 0) {
        throw "Could not find [package] version in $Path."
    }

    if ($lines[$index] -notmatch '^\s*version\s*=\s*"([^"]*)"\s*(#.*)?$') {
        throw "Could not parse [package] version in $Path."
    }

    $value = $Matches[1]
    [void](ConvertFrom-SemVer -Value $value)
    return $value
}

function Set-QuotedVersionLine {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Line,
        [Parameter(Mandatory = $true)]
        [string]$Value
    )

    return [regex]::Replace($Line, 'version\s*=\s*"[^"]*"', ('version = "{0}"' -f $Value), 1)
}

function Set-CargoPackageVersion {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,
        [Parameter(Mandatory = $true)]
        [string]$Value
    )

    $content = Read-Utf8File -Path $Path
    $newline = Get-FileNewline -Content $content
    $lines = Get-TextLines -Content $content
    $index = Get-TomlPackageVersionIndex -Lines $lines
    if ($index -lt 0) {
        throw "Could not update [package] version in $Path."
    }

    $lines[$index] = Set-QuotedVersionLine -Line $lines[$index] -Value $Value
    Write-Utf8File -Path $Path -Content ($lines -join $newline)
}

function Set-LockfilePackageVersion {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,
        [Parameter(Mandatory = $true)]
        [string]$PackageName,
        [Parameter(Mandatory = $true)]
        [string]$Value
    )

    $content = Read-Utf8File -Path $Path
    $newline = Get-FileNewline -Content $content
    $lines = Get-TextLines -Content $content
    $index = Get-LockfilePackageVersionIndex -Lines $lines -PackageName $PackageName
    if ($index -lt 0) {
        throw "Could not update package '$PackageName' version in $Path."
    }

    $lines[$index] = Set-QuotedVersionLine -Line $lines[$index] -Value $Value
    Write-Utf8File -Path $Path -Content ($lines -join $newline)
}

function Read-ReleaseVersion {
    param(
        [Parameter(Mandatory = $true)]
        [string]$CurrentVersion,
        [switch]$Force
    )

    $patch = Get-BumpedVersion -Value $CurrentVersion -Bump Patch
    $minor = Get-BumpedVersion -Value $CurrentVersion -Bump Minor
    $major = Get-BumpedVersion -Value $CurrentVersion -Bump Major

    while ($true) {
        Write-Host "Current version: $CurrentVersion"
        Write-Host "[1] Patch  ($patch) (default)"
        Write-Host "[2] Minor  ($minor)"
        Write-Host "[3] Major  ($major)"
        Write-Host "[4] Custom version"
        $choice = (Read-Host 'Choice').Trim()
        if ([string]::IsNullOrEmpty($choice)) {
            $choice = '1'
        }
        switch ($choice) {
            '1' { return $patch }
            '2' { return $minor }
            '3' { return $major }
            '4' {
                while ($true) {
                    $custom = Normalize-Version -Value (Read-Host 'Custom version')
                    if (-not (Test-SemVer -Value $custom)) {
                        Write-Host "Invalid version '$custom'. Expected semver X.Y.Z with optional -prerelease and +build."
                        continue
                    }
                    if (-not $Force -and (Compare-SemVer -Left $CurrentVersion -Right $custom) -ge 0) {
                        Write-Host "Version '$custom' is not higher than '$CurrentVersion'. Pick a higher version or rerun with -Force."
                        continue
                    }
                    return $custom
                }
            }
            default {
                Write-Host "Invalid selection '$choice'. Enter 1, 2, 3, or 4."
            }
        }
    }
}

Push-Location -LiteralPath $projectRoot
try {
    $cargoToml = Join-Path $projectRoot 'Cargo.toml'
    $cargoLock = Join-Path $projectRoot 'Cargo.lock'
    $packageName = Get-CargoPackageName -Path $cargoToml
    $currentVersion = Get-CargoPackageVersion -Path $cargoToml

    if ($PSBoundParameters.ContainsKey('Prerelease') -and [string]::IsNullOrWhiteSpace($Prerelease)) {
        throw '-Prerelease must not be empty.'
    }
    if ($PSBoundParameters.ContainsKey('Build') -and [string]::IsNullOrWhiteSpace($Build)) {
        throw '-Build must not be empty.'
    }

    if ($PSCmdlet.ParameterSetName -eq 'Exact') {
        $newVersion = Normalize-Version -Value $Version
        [void](ConvertFrom-SemVer -Value $newVersion)
    } elseif ($PSCmdlet.ParameterSetName -eq 'Bump') {
        $bumpArgs = @{
            Value = $currentVersion
            Bump  = $Bump
        }
        if ($PSBoundParameters.ContainsKey('Prerelease')) {
            $bumpArgs['Prerelease'] = $Prerelease
        }
        if ($PSBoundParameters.ContainsKey('Build')) {
            $bumpArgs['Build'] = $Build
        }
        $newVersion = Get-BumpedVersion @bumpArgs
    } else {
        $newVersion = Read-ReleaseVersion -CurrentVersion $currentVersion -Force:$Force
    }

    $precedence = Compare-SemVer -Left $currentVersion -Right $newVersion
    if ($precedence -eq 0) {
        if (-not $Force) {
            throw "New version '$newVersion' has the same precedence as current version '$currentVersion'. Rerun with -Force to rebuild it."
        }
    } elseif ($precedence -gt 0) {
        if (-not $Force) {
            throw "New version '$newVersion' is lower than current version '$currentVersion'. Rerun with -Force to allow the downgrade."
        }
    }

    $parsedNew = ConvertFrom-SemVer -Value $newVersion
    if (-not [string]::IsNullOrEmpty($parsedNew.Build)) {
        Write-Warning "Build metadata '+$($parsedNew.Build)' does not affect precedence and is ignored by Cargo dependency resolution."
    }

    if ($newVersion -ne $currentVersion) {
        if ($PSCmdlet.ShouldProcess($cargoToml, "Set $packageName version to $newVersion")) {
            Set-CargoPackageVersion -Path $cargoToml -Value $newVersion
            $writtenVersion = Get-CargoPackageVersion -Path $cargoToml
            if ($writtenVersion -ne $newVersion) {
                throw "Version write verification failed: Cargo.toml contains '$writtenVersion', expected '$newVersion'."
            }
            if (Test-Path -LiteralPath $cargoLock -PathType Leaf) {
                Set-LockfilePackageVersion -Path $cargoLock -PackageName $packageName -Value $newVersion
            } else {
                Write-Warning "Cargo.lock not found; skipping lockfile sync."
            }
        }
    } else {
        Write-Host "Version unchanged: $currentVersion"
    }

    if ($NoBuild) {
        Write-Host "New version: $newVersion (build skipped)"
        return
    }

    if ($PSCmdlet.ShouldProcess('cargo build --locked --release', 'Build release executable')) {
        & cargo build --locked --release --manifest-path Cargo.toml
        if ($LASTEXITCODE -ne 0) {
            throw 'Release build failed.'
        }
    } else {
        return
    }

    $builtExe = Join-Path $projectRoot 'target\release\ralgruM.exe'
    if (-not (Test-Path -LiteralPath $builtExe -PathType Leaf)) {
        throw "Release executable not found: $builtExe"
    }

    $distDirectory = Join-Path $projectRoot 'dist'
    if ($PSCmdlet.ShouldProcess($distDirectory, 'Stage ralgruM.exe')) {
        New-Item -ItemType Directory -Path $distDirectory -Force | Out-Null
        $distExe = Join-Path $distDirectory 'ralgruM.exe'
        Copy-Item -LiteralPath $builtExe -Destination $distExe -Force

        $distPath = (Resolve-Path -LiteralPath $distExe).Path
        $sizeBytes = (Get-Item -LiteralPath $distPath).Length

        Write-Host "New version: $newVersion"
        Write-Host "Release executable: $distPath"
        Write-Host "File size: $sizeBytes bytes"
    }
} finally {
    Pop-Location
}
