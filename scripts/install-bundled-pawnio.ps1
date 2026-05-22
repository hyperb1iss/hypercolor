param(
    [string]$AssetRoot = "",
    [string]$ModuleDestination = "",
    [switch]$Force,
    [switch]$Silent
)

$ErrorActionPreference = "Stop"

$PawnIoSetupSha256 = "1F519A22E47187F70A1379A48CA604981C4FCF694F4E65B734AAA74A9FBA3032"
$PawnIoModulesZipSha256 = "1149B87F4DC757E72654D5A402863251815EBFC8AD4E3BB030DBCFFB3DE74153"
$RequiredModules = @(
    "SmbusI801.bin",
    "SmbusPIIX4.bin",
    "SmbusNCT6793.bin",
    "IntelMSR.bin",
    "AMDFamily17.bin"
)

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
    Assert-FileHash (Join-Path $AssetRoot "PawnIO_setup.exe") $PawnIoSetupSha256
    Assert-FileHash (Join-Path $AssetRoot "PawnIO.Modules.zip") $PawnIoModulesZipSha256
}

function Expand-VerifiedBundledModules {
    $archive = Join-Path $AssetRoot "PawnIO.Modules.zip"
    $extractRoot = Join-Path ([System.IO.Path]::GetTempPath()) "hypercolor-pawnio-$([System.Guid]::NewGuid().ToString('N'))"
    New-Item -ItemType Directory -Force -Path $extractRoot | Out-Null
    Expand-Archive -LiteralPath $archive -DestinationPath $extractRoot -Force

    foreach ($module in $RequiredModules) {
        $source = Get-ChildItem -Path $extractRoot -Recurse -File -Filter $module |
            Select-Object -First 1
        if ($null -eq $source) {
            throw "Bundled PawnIO module was not found in verified archive: $module"
        }
    }

    return $extractRoot
}

function Copy-BundledModules {
    param(
        [string]$SourceRoot,
        [string]$Destination
    )

    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    foreach ($module in $RequiredModules) {
        $source = Get-ChildItem -Path $SourceRoot -Recurse -File -Filter $module |
            Select-Object -First 1
        if ($null -eq $source) {
            throw "Bundled PawnIO module was not found in verified archive: $module"
        }
        Copy-Item -LiteralPath $source.FullName -Destination (Join-Path $Destination $module) -Force
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

$moduleExtractRoot = Expand-VerifiedBundledModules
try {
    Copy-BundledModules $moduleExtractRoot $ModuleDestination

    if ($pawnIoHome) {
        $sharedModuleDir = Join-Path $pawnIoHome "modules"
        Copy-BundledModules $moduleExtractRoot $sharedModuleDir
    }
} finally {
    if (Test-Path -LiteralPath $moduleExtractRoot) {
        Remove-Item -LiteralPath $moduleExtractRoot -Recurse -Force
    }
}

Write-Host "PawnIO hardware access is ready"
if ($pawnIoHome) {
    Write-Host "  Runtime: $pawnIoHome"
}
Write-Host "  User modules: $ModuleDestination"
