# Populates the local part of the ASIO build toolchain under vendor/:
#   vendor/libclang  : libclang.dll (LLVM 16.0.6) for bindgen, fetched from the
#                      libclang PyPI wheel.
#
# The Steinberg ASIO SDK sources under vendor/asio-sdk are committed to the
# repository and distributed under the GPLv3 option of the Steinberg dual
# license, so nothing needs to be fetched for them. libclang is a normal
# build tool (Apache-2.0 licensed LLVM); installing full LLVM via winget
# works too, this script just avoids needing an admin prompt. The directory
# is read by .cargo/config.toml, which cargo applies to every build here.
#
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File scripts/setup-asio-toolchain.ps1
# Idempotent: an existing libclang.dll is kept.
$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$libclangDir = Join-Path $repoRoot "vendor/libclang"
$libclangPath = Join-Path $libclangDir "libclang.dll"
$wheelSha256 = "daab4a11dae228f1efa9efa3fe638b493b14d8d52c71fb3c7019e2f1df4514c2"
$dllSha256 = "47cb518b1dc80dc73becf2c1f2066f78c471bb087190b87569b96de97f7dc139"

New-Item -ItemType Directory -Force -Path $libclangDir | Out-Null

if (Test-Path -LiteralPath $libclangPath) {
    $installedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $libclangPath).Hash.ToLowerInvariant()
    if ($installedHash -ne $dllSha256) {
        throw "Existing vendor/libclang/libclang.dll has an unexpected SHA-256 hash. Remove it and rerun setup."
    }
    Write-Host "vendor/libclang/libclang.dll already present and verified, skipping."
} else {
    # Bindgen only loads libclang.dll; the PyPI wheel is a convenient
    # redistribution of exactly that file. Pinned to 16.0.6, win_amd64.
    $wheelUrl = "https://files.pythonhosted.org/packages/02/8c/dc970bc00867fe290e8c8a7befa1635af716a9ebdfe3fb9dce0ca4b522ce/libclang-16.0.6-py2.py3-none-win_amd64.whl"
    $tempRoot = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
    $workDir = Join-Path $tempRoot ("ralgrum-libclang-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $workDir | Out-Null
    try {
        $wheelPath = Join-Path $workDir "libclang.whl"
        Write-Host "Downloading libclang wheel..."
        Invoke-WebRequest -Uri $wheelUrl -OutFile $wheelPath
        $downloadedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $wheelPath).Hash.ToLowerInvariant()
        if ($downloadedHash -ne $wheelSha256) { throw "The libclang wheel failed SHA-256 verification." }
        # bsdtar (in-box on Windows 10 1803+) reads zip/wheel archives.
        tar -xf $wheelPath -C $workDir
        if ($LASTEXITCODE -ne 0) { throw "The libclang wheel could not be extracted." }
        $dll = Get-ChildItem -LiteralPath $workDir -Recurse -Filter "libclang.dll" | Select-Object -First 1
        if ($null -eq $dll) { throw "libclang.dll not found inside the wheel" }
        $extractedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $dll.FullName).Hash.ToLowerInvariant()
        if ($extractedHash -ne $dllSha256) { throw "The extracted libclang.dll failed SHA-256 verification." }
        Copy-Item -LiteralPath $dll.FullName -Destination $libclangPath
        Write-Host "Installed verified vendor/libclang/libclang.dll"
    } finally {
        $resolvedWorkDir = [System.IO.Path]::GetFullPath($workDir)
        if ($resolvedWorkDir.StartsWith($tempRoot + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
            Remove-Item -LiteralPath $resolvedWorkDir -Recurse -Force
        }
    }
}

Write-Host "ASIO toolchain ready. cargo build/test in this repository will now find it via .cargo/config.toml."
