param(
    [string]$AssetRoot = "",
    [string]$ModuleDestination = "",
    [switch]$Force,
    [switch]$Silent
)

$ErrorActionPreference = "Stop"

if (-not $AssetRoot) {
    $AssetRoot = Join-Path $PSScriptRoot "pawnio"
}
if (-not $ModuleDestination) {
    $ModuleDestination = Join-Path $env:LOCALAPPDATA "hypercolor\pawnio\modules"
}

function Test-IsAdministrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Resolve-PawnIoHome {
    $programRoots = @($env:ProgramFiles, ${env:ProgramFiles(x86)}) |
        Where-Object { $_ }

    foreach ($root in $programRoots) {
        $candidate = Join-Path $root "PawnIO"
        if (Test-Path -LiteralPath (Join-Path $candidate "PawnIOLib.dll")) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }

    return ""
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

function Assert-FileHash {
    param(
        [string]$Path,
        [string]$ExpectedSha256
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        throw "Expected bundled PawnIO file was not found: $Path"
    }

    $actual = Get-Sha256 $Path
    if ($actual -ne $ExpectedSha256) {
        throw "SHA256 mismatch for $Path; expected $ExpectedSha256, got $actual"
    }
}

function Assert-BundledPawnIoPayload {
    $manifestPath = Join-Path $AssetRoot "manifest.json"
    if (-not (Test-Path -LiteralPath $manifestPath)) {
        return
    }

    $manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
    if ($manifest.pawnio_setup.sha256) {
        Assert-FileHash (Join-Path $AssetRoot "PawnIO_setup.exe") $manifest.pawnio_setup.sha256
    }

    foreach ($module in @($manifest.pawnio_modules.modules)) {
        if ($module.name -and $module.sha256) {
            Assert-FileHash (Join-Path (Join-Path $AssetRoot "modules") $module.name) $module.sha256
        }
    }
}

function Copy-BundledModules {
    param([string]$Destination)

    $sourceDir = Join-Path $AssetRoot "modules"
    if (-not (Test-Path -LiteralPath $sourceDir)) {
        throw "Bundled PawnIO module directory was not found: $sourceDir"
    }

    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    foreach ($module in @("SmbusI801.bin", "SmbusPIIX4.bin", "SmbusNCT6793.bin")) {
        $source = Join-Path $sourceDir $module
        if (-not (Test-Path -LiteralPath $source)) {
            throw "Bundled PawnIO module was not found: $source"
        }
        Copy-Item -LiteralPath $source -Destination (Join-Path $Destination $module) -Force
    }
}

function Quote-ProcessArgument {
    param([string]$Value)

    if ($Value -eq "") {
        return '""'
    }
    if ($Value -notmatch '[\s"]') {
        return $Value
    }

    $escaped = $Value -replace '(\\*)"', '$1$1\"'
    $escaped = $escaped -replace '(\\+)$', '$1$1'
    return '"' + $escaped + '"'
}

Assert-BundledPawnIoPayload

if (-not (Test-IsAdministrator)) {
    $arguments = @(
        "-NoLogo",
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        $PSCommandPath,
        "-AssetRoot",
        $AssetRoot,
        "-ModuleDestination",
        $ModuleDestination
    )
    if ($Force) {
        $arguments += "-Force"
    }
    if ($Silent) {
        $arguments += "-Silent"
    }

    $argumentLine = ($arguments | ForEach-Object { Quote-ProcessArgument $_ }) -join " "
    $process = Start-Process -FilePath "powershell.exe" -ArgumentList $argumentLine -Verb RunAs -Wait -PassThru
    exit $process.ExitCode
}

$setup = Join-Path $AssetRoot "PawnIO_setup.exe"
if (-not (Test-Path -LiteralPath $setup)) {
    throw "Bundled PawnIO installer was not found: $setup"
}

$pawnIoHome = Resolve-PawnIoHome
if (-not $pawnIoHome -or $Force) {
    $installerArgs = if ($Silent) { @("-install", "-silent") } else { @("-install") }
    $process = Start-Process -FilePath $setup -ArgumentList $installerArgs -Wait -PassThru
    $validExitCodes = @(0, 3010)
    if ($validExitCodes -notcontains $process.ExitCode) {
        throw "PawnIO installer failed with exit code $($process.ExitCode)"
    }
    $pawnIoHome = Resolve-PawnIoHome
}

Copy-BundledModules $ModuleDestination

if ($pawnIoHome) {
    $sharedModuleDir = Join-Path $pawnIoHome "modules"
    Copy-BundledModules $sharedModuleDir
}

Write-Host "PawnIO hardware access is ready"
if ($pawnIoHome) {
    Write-Host "  Runtime: $pawnIoHome"
}
Write-Host "  User modules: $ModuleDestination"
