# Populates the local ASIO build toolchain under vendor/:
#   vendor/asio-sdk  : Steinberg ASIO SDK 2.3.3 sources, fetched from Steinberg.
#   vendor/libclang  : libclang.dll (LLVM 16.0.6) for bindgen, fetched from the
#                      libclang PyPI wheel.
#
# Why this exists: the Steinberg ASIO license does not allow redistributing
# the SDK inside this repository, so vendor/ is gitignored and every builder
# fetches their own copy from Steinberg directly. libclang is a normal build
# tool (Apache-2.0 licensed LLVM); installing full LLVM via winget works too,
# this script just avoids needing an admin prompt. Both directories are read
# by .cargo/config.toml, which cargo applies to every build in this repo.
#
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File scripts/setup-asio-toolchain.ps1
# Idempotent: existing files are kept, missing pieces are fetched.
$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$libclangDir = Join-Path $repoRoot "vendor/libclang"
$asioSdkDir = Join-Path $repoRoot "vendor/asio-sdk"

New-Item -ItemType Directory -Force -Path $libclangDir | Out-Null
New-Item -ItemType Directory -Force -Path $asioSdkDir | Out-Null

if (Test-Path (Join-Path $libclangDir "libclang.dll")) {
    Write-Host "vendor/libclang/libclang.dll already present, skipping."
} else {
    # Bindgen only loads libclang.dll; the PyPI wheel is a convenient
    # redistribution of exactly that file. Pinned to 16.0.6, win_amd64.
    $wheelUrl = "https://files.pythonhosted.org/packages/02/8c/dc970bc00867fe290e8c8a7befa1635af716a9ebdfe3fb9dce0ca4b522ce/libclang-16.0.6-py2.py3-none-win_amd64.whl"
    $wheelPath = Join-Path $env:TEMP "libclang-setup.whl"
    Write-Host "Downloading libclang wheel..."
    Invoke-WebRequest -Uri $wheelUrl -OutFile $wheelPath
    $extractDir = Join-Path $env:TEMP "libclang-setup-extract"
    if (Test-Path $extractDir) { Remove-Item -Recurse -Force $extractDir }
    New-Item -ItemType Directory -Force -Path $extractDir | Out-Null
    # bsdtar (in-box on Windows 10 1803+) reads zip/wheel archives.
    tar -xf $wheelPath -C $extractDir
    $dll = Get-ChildItem -Path $extractDir -Recurse -Filter "libclang.dll" | Select-Object -First 1
    if ($null -eq $dll) { throw "libclang.dll not found inside the wheel" }
    Copy-Item $dll.FullName (Join-Path $libclangDir "libclang.dll") -Force
    Remove-Item -Recurse -Force $extractDir
    Write-Host "Installed vendor/libclang/libclang.dll"
}

if (Test-Path (Join-Path $asioSdkDir "asio")) {
    Write-Host "vendor/asio-sdk already populated, skipping."
} else {
    # The same URL the asio-sys build script itself uses. The zip carries the
    # Steinberg ASIO Licensing Agreement; using it means agreeing to it.
    $sdkUrl = "https://www.steinberg.net/asiosdk"
    $zipPath = Join-Path $env:TEMP "asio-sdk-setup.zip"
    Write-Host "Downloading Steinberg ASIO SDK..."
    Invoke-WebRequest -Uri $sdkUrl -OutFile $zipPath
    $extractDir = Join-Path $env:TEMP "asio-sdk-setup-extract"
    if (Test-Path $extractDir) { Remove-Item -Recurse -Force $extractDir }
    New-Item -ItemType Directory -Force -Path $extractDir | Out-Null
    Expand-Archive -Path $zipPath -DestinationPath $extractDir
    $inner = Get-ChildItem -Path $extractDir -Directory | Where-Object { $_.Name -like "ASIOSDK*" } | Select-Object -First 1
    if ($null -eq $inner) { throw "ASIOSDK folder not found inside the SDK archive" }
    Copy-Item (Join-Path $inner.FullName "*") $asioSdkDir -Recurse -Force
    Remove-Item -Recurse -Force $extractDir
    Write-Host "Installed vendor/asio-sdk from $($inner.Name)"
}

Write-Host "ASIO toolchain ready. cargo build/test in this repository will now find it via .cargo/config.toml."
