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

New-Item -ItemType Directory -Force -Path $libclangDir | Out-Null

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

Write-Host "ASIO toolchain ready. cargo build/test in this repository will now find it via .cargo/config.toml."
