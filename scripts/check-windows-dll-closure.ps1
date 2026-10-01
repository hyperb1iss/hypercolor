param(
    [Parameter(Mandatory = $true)]
    [string] $Installer
)

# Extract a built NSIS installer and prove every shipped binary can load
# on a clean Windows install. A build runner is the wrong place to trust
# DLL resolution: its System32 and PATH carry the VC++ redistributable
# that consumer machines lack, so a missing app-local runtime only shows
# up as a daemon that never starts on a user's machine.

$ErrorActionPreference = 'Stop'

# Imports that must sit beside the binary that loads them, never resolved
# from the runner's system directories.
$AppLocalPattern = '^(msvcp140(_\d|_atomic_wait|_codecvt_ids)?|vcruntime140(_1)?|concrt140|vccorlib140|vcomp140)\.dll$'

function Get-PeImports {
    param([string] $Path)

    $objdump = Get-Command llvm-objdump -ErrorAction SilentlyContinue
    if ($null -eq $objdump) {
        $candidate = Join-Path $env:ProgramFiles 'LLVM\bin\llvm-objdump.exe'
        if (Test-Path -LiteralPath $candidate) {
            $objdump = Get-Command $candidate
        }
    }
    if ($null -eq $objdump) {
        throw 'llvm-objdump is required to read PE import tables'
    }
    & $objdump.Source -p $Path |
        Select-String -CaseSensitive -Pattern 'DLL Name:\s*(\S+)' |
        ForEach-Object { $_.Matches[0].Groups[1].Value.ToLowerInvariant() } |
        Sort-Object -Unique
}

if (-not (Test-Path -LiteralPath $Installer)) {
    throw "installer not found: $Installer"
}

$extractDir = Join-Path ([System.IO.Path]::GetTempPath()) "hypercolor-dll-closure-$PID"
if (Test-Path -LiteralPath $extractDir) {
    Remove-Item -LiteralPath $extractDir -Recurse -Force
}
& 7z x -y "-o$extractDir" $Installer | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "7z could not extract $Installer"
}

$system32 = Join-Path $env:WINDIR 'System32'
$binaries = Get-ChildItem -LiteralPath $extractDir -Recurse -File -Include '*.exe', '*.dll' |
    Where-Object { $_.FullName -notmatch '\\\$PLUGINSDIR\\' -and $_.Name -ne 'uninstall.exe' -and $_.Name -ne 'PawnIO_setup.exe' }

$failures = @()
foreach ($binary in $binaries) {
    $relative = $binary.FullName.Substring($extractDir.Length + 1)
    foreach ($import in Get-PeImports $binary.FullName) {
        $beside = Test-Path -LiteralPath (Join-Path $binary.DirectoryName $import)
        if ($import -match $AppLocalPattern) {
            if (-not $beside) {
                $failures += "$relative imports $import, which must ship beside it"
            }
            continue
        }
        if ($beside -or $import -like 'api-ms-*' -or $import -like 'ext-ms-*') {
            continue
        }
        if (-not (Test-Path -LiteralPath (Join-Path $system32 $import))) {
            $failures += "$relative imports $import, which is neither bundled nor a Windows system DLL"
        }
    }
}

Remove-Item -LiteralPath $extractDir -Recurse -Force

if ($failures.Count -gt 0) {
    $failures | ForEach-Object { Write-Host "::error::$_" }
    throw "$($failures.Count) unresolved DLL import(s) in $Installer"
}
Write-Host "DLL closure ok: $($binaries.Count) binaries in $(Split-Path -Leaf $Installer)"
