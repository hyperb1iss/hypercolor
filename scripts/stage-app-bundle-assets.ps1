param(
    [string] $Profile = 'release',
    [string] $Target = '',
    [switch] $SkipPawnIo
)

$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot
$AppRoot = Join-Path $RepoRoot 'crates\hypercolor-app'
$StageBin = Join-Path $AppRoot 'binaries'
$StageResources = Join-Path $AppRoot 'resources'

function Get-HypercolorHostTriple {
    $hostTuple = & rustc --print host-tuple 2>$null
    if ($LASTEXITCODE -eq 0 -and $hostTuple) {
        return $hostTuple.Trim()
    }

    $verbose = & rustc -vV
    $hostLine = $verbose | Where-Object { $_ -like 'host: *' } | Select-Object -First 1
    if (-not $hostLine) {
        throw 'failed to determine Rust host triple'
    }
    return ($hostLine -replace '^host:\s*', '').Trim()
}

function Assert-UnderRepo {
    param([string] $Path)

    $resolvedRepo = [System.IO.Path]::GetFullPath($RepoRoot).TrimEnd(
        [System.IO.Path]::DirectorySeparatorChar,
        [System.IO.Path]::AltDirectorySeparatorChar
    )
    $resolvedPath = [System.IO.Path]::GetFullPath($Path).TrimEnd(
        [System.IO.Path]::DirectorySeparatorChar,
        [System.IO.Path]::AltDirectorySeparatorChar
    )
    $repoPrefix = "$resolvedRepo$([System.IO.Path]::DirectorySeparatorChar)"
    if (
        $resolvedPath -ne $resolvedRepo -and
        -not $resolvedPath.StartsWith($repoPrefix, [System.StringComparison]::OrdinalIgnoreCase)
    ) {
        throw "refusing to modify path outside repository: $resolvedPath"
    }
}

function Reset-Directory {
    param([string] $Path)

    Assert-UnderRepo $Path
    if (Test-Path -LiteralPath $Path) {
        Remove-Item -LiteralPath $Path -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $Path | Out-Null
}

function Copy-DirectoryContents {
    param(
        [string] $Source,
        [string] $Destination
    )

    Reset-Directory $Destination
    Get-ChildItem -LiteralPath $Source -Force |
        ForEach-Object {
            Copy-Item -LiteralPath $_.FullName -Destination $Destination -Recurse -Force
        }
}

function Copy-Sidecar {
    param([string] $Name)

    $source = Join-Path $ProfileDir "$Name$Exe"
    if (-not (Test-Path -LiteralPath $source)) {
        throw "missing built binary: $source; build release binaries before staging app bundle assets"
    }

    $targetPath = Join-Path $StageBin "$Name-$Target$Exe"
    Copy-Item -LiteralPath $source -Destination $targetPath -Force
}

function Copy-ToolScript {
    param([string] $Name)

    $source = Join-Path $RepoRoot "scripts\$Name"
    if (-not (Test-Path -LiteralPath $source)) {
        throw "missing tool script: $source"
    }

    Copy-Item -LiteralPath $source -Destination (Join-Path $StageResources 'tools') -Force
}

function Copy-WindowsToolBinary {
    param([string] $Name)

    $source = Join-Path $ProfileDir "$Name$Exe"
    if (-not (Test-Path -LiteralPath $source)) {
        throw "missing built Windows tool binary: $source; build release binaries before staging app bundle assets"
    }

    Copy-Item -LiteralPath $source -Destination (Join-Path (Join-Path $StageResources 'tools') "$Name$Exe") -Force
}

function Test-WindowsTarget {
    $Target -like '*windows*' -or $Target -like '*-pc-windows-*'
}

Set-Location $RepoRoot

$HostTarget = Get-HypercolorHostTriple
if (-not $Target) {
    $Target = $HostTarget
}

$TargetDir = if ($env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR
} else {
    Join-Path $RepoRoot 'target'
}

$ProfileDir = Join-Path $TargetDir $Profile
if ($Target -ne $HostTarget) {
    $ProfileDir = Join-Path (Join-Path $TargetDir $Target) $Profile
}

$Exe = if ($Target -like '*windows*' -or $Target -like '*-pc-windows-*') { '.exe' } else { '' }

Reset-Directory $StageBin
New-Item -ItemType Directory -Force `
    -Path (Join-Path $StageResources 'ui'), `
          (Join-Path $StageResources 'effects\bundled'), `
          (Join-Path $StageResources 'tools') | Out-Null

Copy-Sidecar 'hypercolor-daemon'
Copy-Sidecar 'hypercolor'

$uiDist = Join-Path $RepoRoot 'crates\hypercolor-ui\dist'
if (Test-Path -LiteralPath $uiDist) {
    Copy-DirectoryContents $uiDist (Join-Path $StageResources 'ui')
} else {
    Write-Warning 'crates/hypercolor-ui/dist not found; UI resources left as-is'
}

$effectsDist = Join-Path $RepoRoot 'effects\hypercolor'
if (Test-Path -LiteralPath $effectsDist) {
    Copy-DirectoryContents $effectsDist (Join-Path $StageResources 'effects\bundled')
} else {
    Write-Warning 'effects/hypercolor not found; bundled effects left as-is'
}

Copy-ToolScript 'install-windows-service.ps1'
Copy-ToolScript 'uninstall-windows-service.ps1'
Copy-ToolScript 'diagnose-windows.ps1'
Copy-ToolScript 'install-windows-smbus-service.ps1'
Copy-ToolScript 'install-pawnio-modules.ps1'
Copy-ToolScript 'install-bundled-pawnio.ps1'

if (Test-WindowsTarget) {
    Copy-WindowsToolBinary 'hypercolor-smbus-service'

    if (-not $SkipPawnIo) {
        & (Join-Path $RepoRoot 'scripts\fetch-pawnio-assets.ps1') `
            -Destination (Join-Path (Join-Path $StageResources 'tools') 'pawnio')
    }
}

Write-Host "staged hypercolor-app bundle assets for $Target ($Profile)"
