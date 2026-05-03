$ErrorActionPreference = 'Stop'

$CommandArgs = @($args | ForEach-Object { [string] $_ })

$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

function Add-PathPrefix {
    param([string] $Path)

    if ((Test-Path $Path) -and (($env:Path -split ';') -notcontains $Path)) {
        $env:Path = "$Path;$env:Path"
    }
}

function Enter-HypercolorVsDevShell {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path $vswhere)) {
        throw 'vswhere.exe not found. Install Visual Studio 2026 Build Tools with the C++ desktop workload.'
    }

    $vs = & $vswhere `
        -latest `
        -products * `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -requires Microsoft.VisualStudio.Component.Windows11SDK.26100 `
        -property installationPath
    if (-not $vs) {
        throw 'No Visual Studio C++ toolchain found. Install the C++ desktop workload and Windows 11 SDK 26100.'
    }

    $devShell = Join-Path $vs 'Common7\Tools\Microsoft.VisualStudio.DevShell.dll'
    Import-Module $devShell
    Enter-VsDevShell -VsInstallPath $vs -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64' | Out-Null
}

function Initialize-HypercolorCargoCache {
    $cacheRoot = if ($env:HYPERCOLOR_CACHE_DIR) {
        $env:HYPERCOLOR_CACHE_DIR
    } else {
        Join-Path $env:USERPROFILE '.cache\hypercolor'
    }

    if (-not $env:CARGO_TARGET_DIR) {
        $env:CARGO_TARGET_DIR = Join-Path $cacheRoot 'target'
    }
    if (-not $env:MOZBUILD_STATE_PATH) {
        $env:MOZBUILD_STATE_PATH = Join-Path $cacheRoot 'mozbuild'
    }

    New-Item -ItemType Directory -Force -Path $env:CARGO_TARGET_DIR, $env:MOZBUILD_STATE_PATH | Out-Null

    if ($CommandArgs.Count -eq 0) {
        $script:CommandArgs = @('cargo', 'build', '--workspace')
    }

    $usesReleaseLikeProfile = $false
    for ($i = 0; $i -lt $CommandArgs.Count; $i += 1) {
        $arg = $CommandArgs[$i]
        if ($arg -eq '--release') {
            $usesReleaseLikeProfile = $true
        } elseif ($arg -eq '--profile' -and ($i + 1) -lt $CommandArgs.Count) {
            $usesReleaseLikeProfile = $CommandArgs[$i + 1] -in @('release', 'bench')
        } elseif ($arg -match '^--profile=(release|bench)$') {
            $usesReleaseLikeProfile = $true
        }
    }

    $sccache = Get-Command sccache.exe -ErrorAction SilentlyContinue
    if ($null -ne $sccache -and $usesReleaseLikeProfile) {
        if (-not $env:SCCACHE_DIR) {
            $env:SCCACHE_DIR = Join-Path $cacheRoot 'sccache'
        }
        New-Item -ItemType Directory -Force -Path $env:SCCACHE_DIR | Out-Null
        if (-not $env:RUSTC_WRAPPER) {
            $env:RUSTC_WRAPPER = $sccache.Source
        }
        $env:CARGO_BUILD_INCREMENTAL = 'false'
        $env:CARGO_PROFILE_RELEASE_INCREMENTAL = 'false'
        $env:CARGO_PROFILE_BENCH_INCREMENTAL = 'false'
        Write-Host "[cargo-cache] sccache enabled for Rust compilation"
        Write-Host "[cargo-cache] SCCACHE_DIR=$env:SCCACHE_DIR"
    } else {
        if (-not $env:CARGO_INCREMENTAL) {
            $env:CARGO_INCREMENTAL = '1'
        }
        Write-Host "[cargo-cache] using Cargo incremental compilation on Windows"
        Write-Host "[cargo-cache] CARGO_INCREMENTAL=$env:CARGO_INCREMENTAL"
    }

    Write-Host "[cargo-cache] CARGO_TARGET_DIR=$env:CARGO_TARGET_DIR"
    Write-Host "[cargo-cache] MOZBUILD_STATE_PATH=$env:MOZBUILD_STATE_PATH"
}

function Initialize-HypercolorNativeTools {
    $nasmDir = 'C:\Program Files\NASM'
    $nasmExe = Join-Path $nasmDir 'nasm.exe'
    if (Test-Path $nasmExe) {
        Add-PathPrefix $nasmDir
        $env:ASM_NASM = $nasmExe
    }
}

function Invoke-HypercolorCargoCommand {
    $exe = $CommandArgs[0]
    $args = if ($CommandArgs.Count -gt 1) {
        $CommandArgs[1..($CommandArgs.Count - 1)]
    } else {
        @()
    }

    Write-Host "[cargo-cache] running: $($CommandArgs -join ' ')"
    & $exe @args
    exit $LASTEXITCODE
}

Enter-HypercolorVsDevShell
Initialize-HypercolorNativeTools
Initialize-HypercolorCargoCache
Invoke-HypercolorCargoCommand
