param(
    [string]$Version = "0.2.5",
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

Write-Host "Downloading PawnIO modules $Version"
Invoke-WebRequest $url -OutFile $zip

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
