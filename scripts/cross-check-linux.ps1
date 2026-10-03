# Cross-checks the Linux targets from a Windows host using Zig as the C
# compiler. This is a development helper, not CI: real CI builds on Linux
# runners. Requires a one-time setup:
#   - Zig 0.14.1 in C:\Users\Kekko\zig-x86_64-windows-0.14.1\zig.exe
#   - Wrapper exes in C:\Users\Kekko\cross\ (zigwrap.exe translating the
#     target triple, pc.exe answering pkg-config probes)
param(
    [ValidateSet('aarch64-unknown-linux-gnu', 'x86_64-unknown-linux-gnu')]
    [string]$Target = 'aarch64-unknown-linux-gnu',
    [switch]$AllTargets
)

$ErrorActionPreference = 'Stop'

$zigDir = 'C:\Users\Kekko\zig-x86_64-windows-0.14.1\zig.exe'
$crossDir = 'C:\Users\Kekko\cross'
if (-not (Test-Path $zigDir) -or -not (Test-Path "$crossDir\zigwrap.exe")) {
    throw "Cross harness is not set up under $crossDir. See scripts/cross-check-linux.ps1 header."
}

$targetEnv = $Target.Replace('-', '_')
[Environment]::SetEnvironmentVariable("CC_$targetEnv", "$crossDir\zig-cc.bat", 'Process')
[Environment]::SetEnvironmentVariable("CXX_$targetEnv", "$crossDir\zig-cxx.bat", 'Process')
[Environment]::SetEnvironmentVariable("AR_$targetEnv", "$crossDir\zig-ar.bat", 'Process')
$env:PKG_CONFIG = "$crossDir\pc.exe"
$env:PKG_CONFIG_ALLOW_CROSS = '1'
$env:AWS_LC_SYS_CMAKE_BUILDER = '0'

if ($AllTargets) {
    cargo check --all-targets --target $Target
} else {
    cargo check --target $Target
}
