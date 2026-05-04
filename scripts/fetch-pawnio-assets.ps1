param(
    [string]$Destination = "",
    [string]$CacheDir = ""
)

$ErrorActionPreference = "Stop"

$PawnIoSetupVersion = "2.2.0"
$PawnIoSetupSha256 = "1F519A22E47187F70A1379A48CA604981C4FCF694F4E65B734AAA74A9FBA3032"
$PawnIoModulesVersion = "0.2.5"
$PawnIoModulesSha256 = "1149B87F4DC757E72654D5A402863251815EBFC8AD4E3BB030DBCFFB3DE74153"
$RequiredModules = @(
    "SmbusI801.bin",
    "SmbusPIIX4.bin",
    "SmbusNCT6793.bin"
)

$RepoRoot = Split-Path -Parent $PSScriptRoot
if (-not $Destination) {
    $Destination = Join-Path $RepoRoot "crates\hypercolor-app\resources\tools\pawnio"
}
if (-not $CacheDir) {
    $CacheDir = Join-Path $RepoRoot "target\pawnio"
}

function Assert-UnderRepo {
    param([string]$Path)

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
    param([string]$Path)

    Assert-UnderRepo $Path
    if (Test-Path -LiteralPath $Path) {
        Remove-Item -LiteralPath $Path -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $Path | Out-Null
}

function Get-Sha256 {
    param([string]$Path)

    if (Get-Command "Get-FileHash" -ErrorAction SilentlyContinue) {
        return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToUpperInvariant()
    }

    $resolved = (Resolve-Path -LiteralPath $Path).Path
    $stream = [System.IO.File]::OpenRead($resolved)
    try {
        $sha256 = [System.Security.Cryptography.SHA256]::Create()
        try {
            $hash = $sha256.ComputeHash($stream)
        } finally {
            $sha256.Dispose()
        }
    } finally {
        $stream.Dispose()
    }

    return -join ($hash | ForEach-Object { $_.ToString("X2") })
}

function Save-VerifiedFile {
    param(
        [string]$Url,
        [string]$Path,
        [string]$ExpectedSha256
    )

    $download = $true
    if (Test-Path -LiteralPath $Path) {
        $actual = Get-Sha256 $Path
        $download = $actual -ne $ExpectedSha256
    }

    if ($download) {
        Invoke-WebRequest -Uri $Url -OutFile $Path
    }

    $actual = Get-Sha256 $Path
    if ($actual -ne $ExpectedSha256) {
        throw "SHA256 mismatch for $Path; expected $ExpectedSha256, got $actual"
    }
}

Set-Location $RepoRoot
Assert-UnderRepo $Destination
Assert-UnderRepo $CacheDir

New-Item -ItemType Directory -Force -Path $CacheDir | Out-Null
Reset-Directory $Destination

$setupUrl = "https://github.com/namazso/PawnIO.Setup/releases/download/$PawnIoSetupVersion/PawnIO_setup.exe"
$modulesUrl = "https://github.com/namazso/PawnIO.Modules/releases/download/$PawnIoModulesVersion/release_$($PawnIoModulesVersion -replace '\.', '_').zip"
$setupCache = Join-Path $CacheDir "PawnIO_setup-$PawnIoSetupVersion.exe"
$modulesCache = Join-Path $CacheDir "PawnIO.Modules-$PawnIoModulesVersion.zip"

Save-VerifiedFile $setupUrl $setupCache $PawnIoSetupSha256
Save-VerifiedFile $modulesUrl $modulesCache $PawnIoModulesSha256

Copy-Item -LiteralPath $setupCache -Destination (Join-Path $Destination "PawnIO_setup.exe") -Force

$extractRoot = Join-Path $CacheDir "modules-$PawnIoModulesVersion"
Reset-Directory $extractRoot
Expand-Archive -LiteralPath $modulesCache -DestinationPath $extractRoot -Force

$moduleDestination = Join-Path $Destination "modules"
New-Item -ItemType Directory -Force -Path $moduleDestination | Out-Null

$stagedModules = @()
foreach ($module in $RequiredModules) {
    $source = Get-ChildItem -Path $extractRoot -Recurse -File -Filter $module |
        Select-Object -First 1
    if ($null -eq $source) {
        throw "PawnIO module $module was not found in release archive"
    }
    $modulePath = Join-Path $moduleDestination $module
    Copy-Item -LiteralPath $source.FullName -Destination $modulePath -Force
    $stagedModules += [ordered]@{
        name = $module
        sha256 = Get-Sha256 $modulePath
    }
}

$license = Get-ChildItem -Path $extractRoot -Recurse -File -Filter "COPYING" |
    Select-Object -First 1
if ($null -ne $license) {
    Copy-Item -LiteralPath $license.FullName -Destination (Join-Path $moduleDestination "COPYING") -Force
}

$manifest = [ordered]@{
    pawnio_setup = [ordered]@{
        version = $PawnIoSetupVersion
        url = $setupUrl
        sha256 = $PawnIoSetupSha256
    }
    pawnio_modules = [ordered]@{
        version = $PawnIoModulesVersion
        url = $modulesUrl
        sha256 = $PawnIoModulesSha256
        installed_modules = $RequiredModules
        modules = $stagedModules
    }
}
$manifest |
    ConvertTo-Json -Depth 8 |
    Set-Content -Encoding UTF8 -NoNewline (Join-Path $Destination "manifest.json")

Write-Host "staged PawnIO $PawnIoSetupVersion and SMBus modules $PawnIoModulesVersion to $Destination"
