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

function Assert-HypercolorRustToolchain {
    # A second Rust on PATH is the worst kind of broken: it fails late and
    # selectively. Cached crates check fine, so a build looks healthy right up
    # until something needs a fresh link and dies inside a dependency's build
    # script, or hits E0514 rustc-mismatch errors against artifacts the pinned
    # toolchain left in target/. Chocolatey's `rust` package is the usual
    # culprit on Windows: it installs a GNU-host toolchain under
    # C:\ProgramData\chocolatey\bin, which is in the *system* PATH and
    # therefore beats ~\.cargo\bin in the user PATH. Fail here, with the
    # actual resolved paths, instead of 200 lines into a mingw linker error.
    $rustc = Get-Command rustc -ErrorAction SilentlyContinue
    if (-not $rustc) {
        throw 'rustc not found on PATH. Install Rust with rustup: https://rustup.rs/'
    }

    $version = & rustc -vV
    $host_ = ($version | Select-String '^host: (.+)$').Matches.Groups[1].Value

    if ($host_ -ne 'x86_64-pc-windows-msvc') {
        throw @"
Wrong Rust host toolchain: $host_ (expected x86_64-pc-windows-msvc)
  rustc resolves to: $($rustc.Source)
Hypercolor builds against MSVC. A GNU-host Rust ahead of rustup on PATH is
almost always Chocolatey's `rust` package; remove it so rustup wins:
  choco uninstall rust -y
"@
    }
}

function Initialize-HypercolorCargoCache {
    $cacheRoot = if ($env:HYPERCOLOR_CACHE_DIR) {
        $env:HYPERCOLOR_CACHE_DIR
    } else {
        Join-Path $env:USERPROFILE '.cache\hypercolor'
    }

    if (-not $env:CARGO_TARGET_DIR) {
        $env:CARGO_TARGET_DIR = Join-Path $RepoRoot 'target'
    }
    if (-not $env:MOZBUILD_STATE_PATH) {
        $env:MOZBUILD_STATE_PATH = Join-Path $cacheRoot 'mozbuild'
    }
    if (-not $env:CMAKE_TOOLCHAIN_FILE) {
        $env:CMAKE_TOOLCHAIN_FILE = Join-Path $PSScriptRoot 'cmake\windows-msvc-dynamic-crt.cmake'
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
            if ($CommandArgs[$i + 1] -in @('release', 'bench')) {
                $usesReleaseLikeProfile = $true
            }
        } elseif ($arg -match '^--profile=(release|bench)$') {
            $usesReleaseLikeProfile = $true
        }
    }

    # sccache is the cross-worktree sharing layer: every worktree keeps its own
    # target dir (so parallel agent builds never contend on Cargo's target
    # lock), while identical compiles hit one bounded cache under the shared
    # cache root. sccache and incremental compilation are mutually exclusive
    # (sccache 0.17 hard-errors on either the env var or -Cincremental), so
    # the wrapper picks per command: codegen-heavy tree ops go through
    # sccache; metadata-only ops (check/clippy) keep incremental because
    # sccache cannot cache --emit=metadata units at all.
    $cargoSubcommand = ''
    if ($CommandArgs.Count -gt 1 -and $CommandArgs[0] -match 'cargo(\.exe)?$') {
        for ($i = 1; $i -lt $CommandArgs.Count; $i += 1) {
            if ($CommandArgs[$i] -notmatch '^[-+]') {
                $cargoSubcommand = $CommandArgs[$i]
                break
            }
        }
    }
    # `run` stays incremental: it is the edit-run iteration loop, and a
    # measured edit-rebuild of hypercolor-core is ~45s non-incremental vs
    # ~11s incremental. Whole-tree ops win with sccache instead.
    $sccacheSubcommands = @('build', 'test', 'bench', 'nextest')
    $forceSccache = $env:HYPERCOLOR_FORCE_SCCACHE -in @('1', 'true', 'TRUE')
    $wantsSccache = $forceSccache -or $usesReleaseLikeProfile -or ($cargoSubcommand -in $sccacheSubcommands)

    $sccache = Get-Command sccache.exe -ErrorAction SilentlyContinue
    $disableSccache = ($env:HYPERCOLOR_NO_SCCACHE -in @('1', 'true', 'TRUE')) -or
        ($env:HYPERCOLOR_ITERATE -in @('1', 'true', 'TRUE')) -or
        ($env:CARGO_INCREMENTAL -and $env:CARGO_INCREMENTAL -ne '0')
    if ($null -ne $sccache -and $wantsSccache -and -not $disableSccache) {
        if (-not $env:SCCACHE_DIR) {
            $env:SCCACHE_DIR = Join-Path $cacheRoot 'sccache'
        }
        New-Item -ItemType Directory -Force -Path $env:SCCACHE_DIR | Out-Null
        if (-not $env:SCCACHE_CACHE_SIZE) {
            $env:SCCACHE_CACHE_SIZE = if ($env:HYPERCOLOR_SCCACHE_SIZE) {
                $env:HYPERCOLOR_SCCACHE_SIZE
            } else {
                '75G'
            }
        }
        if (-not $env:RUSTC_WRAPPER) {
            $env:RUSTC_WRAPPER = $sccache.Source
        }
        # C/C++ caching: mozangle (ANGLE) builds through the cc crate and
        # turbojpeg through CMake; both honor these and route cl.exe through
        # sccache, which is where repeat ANGLE builds go to die.
        if (-not $env:CMAKE_C_COMPILER_LAUNCHER) {
            $env:CMAKE_C_COMPILER_LAUNCHER = $sccache.Source
        }
        if (-not $env:CMAKE_CXX_COMPILER_LAUNCHER) {
            $env:CMAKE_CXX_COMPILER_LAUNCHER = $sccache.Source
        }
        if (-not $env:CC) {
            $env:CC = 'sccache cl'
        }
        if (-not $env:CXX) {
            $env:CXX = 'sccache cl'
        }
        $env:CARGO_INCREMENTAL = '0'
        Write-Host "[cargo-cache] sccache mode: Rust + C/C++ cached, incremental off (cap $env:SCCACHE_CACHE_SIZE)"
        Write-Host "[cargo-cache] SCCACHE_DIR=$env:SCCACHE_DIR (HYPERCOLOR_NO_SCCACHE=1 or HYPERCOLOR_ITERATE=1 to disable)"
    } else {
        # An ambient sccache RUSTC_WRAPPER combined with incremental
        # hard-fails every compile; drop it rather than let the build die.
        if ($env:RUSTC_WRAPPER -and (Split-Path -Leaf $env:RUSTC_WRAPPER) -like 'sccache*') {
            Remove-Item Env:RUSTC_WRAPPER -ErrorAction SilentlyContinue
            Write-Host "[cargo-cache] unset ambient sccache RUSTC_WRAPPER for incremental mode"
        }
        if (-not $env:CARGO_INCREMENTAL) {
            $env:CARGO_INCREMENTAL = '1'
        }
        if ($null -eq $sccache) {
            Write-Host "[cargo-cache] sccache not found; run 'cargo install --locked sccache' for shared build caching"
        }
        Write-Host "[cargo-cache] incremental mode: CARGO_INCREMENTAL=$env:CARGO_INCREMENTAL"
    }

    # rust-lld ships with the toolchain and links large statically-linked
    # binaries (daemon + Servo + mozjs) far faster than link.exe. Dev-loop
    # only: release-like builds keep link.exe so shipped artifacts (CI
    # sidecars, the installer, app bundles built outside this wrapper) all
    # come off the same linker.
    $disableFastLink = $env:HYPERCOLOR_NO_FAST_LINK -in @('1', 'true', 'TRUE')
    if (-not $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER -and
        -not $disableFastLink -and
        -not $usesReleaseLikeProfile) {
        $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER = 'rust-lld'
        Write-Host "[cargo-cache] linker: rust-lld (HYPERCOLOR_NO_FAST_LINK=1 to disable)"
    }

    Write-Host "[cargo-cache] CARGO_TARGET_DIR=$env:CARGO_TARGET_DIR"
    Write-Host "[cargo-cache] MOZBUILD_STATE_PATH=$env:MOZBUILD_STATE_PATH"
    Write-Host "[cargo-cache] CMAKE_TOOLCHAIN_FILE=$env:CMAKE_TOOLCHAIN_FILE"
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
Assert-HypercolorRustToolchain
Initialize-HypercolorNativeTools
Initialize-HypercolorCargoCache
Invoke-HypercolorCargoCommand
