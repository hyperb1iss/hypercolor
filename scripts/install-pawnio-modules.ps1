param(
    [string]$Version = "0.2.5",
    [string]$ExpectedSha256 = "1149B87F4DC757E72654D5A402863251815EBFC8AD4E3BB030DBCFFB3DE74153",
    [string]$Destination = "$env:LOCALAPPDATA\hypercolor\pawnio\modules"
)

$ErrorActionPreference = "Stop"

$modules = @(
    "SmbusI801.bin",
    "SmbusPIIX4.bin",
    "SmbusNCT6793.bin"
)

$zip = Join-Path $env:TEMP "hypercolor-pawnio-modules-$Version.zip"
$url = "https://github.com/namazso/PawnIO.Modules/releases/download/$Version/release_$($Version -replace '\.', '_').zip"
$extractRoot = Join-Path $env:TEMP "hypercolor-pawnio-modules-$Version"

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

Write-Host "Downloading PawnIO modules $Version"
Invoke-WebRequest $url -OutFile $zip

$actualSha256 = Get-Sha256 $zip
if ($actualSha256 -ne $ExpectedSha256) {
    throw "SHA256 mismatch for $zip; expected $ExpectedSha256, got $actualSha256"
}

if (Test-Path $extractRoot) {
    Remove-Item -LiteralPath $extractRoot -Recurse -Force
}
New-Item -ItemType Directory -Path $extractRoot | Out-Null
Expand-Archive -Path $zip -DestinationPath $extractRoot -Force

New-Item -ItemType Directory -Path $Destination -Force | Out-Null

foreach ($module in $modules) {
    $source = Get-ChildItem -Path $extractRoot -Recurse -Filter $module | Select-Object -First 1
    if ($null -eq $source) {
        throw "PawnIO module $module was not found in release archive"
    }

    Copy-Item -LiteralPath $source.FullName -Destination (Join-Path $Destination $module) -Force
    Write-Host "Installed $module"
}

Write-Host "PawnIO modules installed to $Destination"
