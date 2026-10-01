param(
    [Parameter(Mandatory = $true)]
    [string] $Installer
)

# Extract a built NSIS installer and prove every shipped binary can load
# on a clean Windows install. A build runner is the wrong place to trust
# DLL resolution: its System32 and PATH carry Microsoft C and C++ runtimes
# (current, legacy, and debug) that consumer machines lack, so a missing
# app-local runtime only shows up as a daemon that never starts on a
# user's machine. Runtime imports must therefore ship beside the binary
# that loads them; every other import must be bundled, an API set, or
# present in System32.

$ErrorActionPreference = 'Stop'

# The Microsoft C and C++ runtime family, including legacy and debug
# builds. These never count as resolved through the runner's System32.
$RuntimePattern = '^(msvc[pr]\d+d?(_\w+)?|vcruntime\d+(_1)?d?|concrt\d+d?|vccorlib\d+d?|vcomp\d+d?|vcamp\d+d?|mfc\d+\w*|ucrtbased)\.dll$'

# A guard that sees no binaries proves nothing, so the core of the bundle
# must be present.
$RequiredBinaries = @(
    'hypercolor-app.exe',
    'hypercolor-daemon.exe',
    'hypercolor.exe',
    'tools\hypercolor-smbus-service.exe',
    'tools\hypercolor-windows-helper.exe'
)

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
    # llvm-objdump prints imports as "DLL Name:" and the export table's own
    # module name as "DLL name:", so the match must stay case-sensitive.
    $dump = & $objdump.Source -p $Path
    if ($LASTEXITCODE -ne 0) {
        throw "llvm-objdump could not read $Path"
    }
    @($dump |
        Select-String -CaseSensitive -Pattern 'DLL Name:\s*(\S+)' |
        ForEach-Object { $_.Matches[0].Groups[1].Value.ToLowerInvariant() } |
        Sort-Object -Unique)
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
foreach ($required in $RequiredBinaries) {
    if (-not (Test-Path -LiteralPath (Join-Path $extractDir $required))) {
        $failures += "$required is missing from the installer"
    }
}
foreach ($binary in $binaries) {
    $relative = $binary.FullName.Substring($extractDir.Length + 1)
    $imports = Get-PeImports $binary.FullName
    if ($imports.Count -eq 0) {
        $failures += "$relative reports no imports; its import table could not be read"
        continue
    }
    foreach ($import in $imports) {
        $beside = Test-Path -LiteralPath (Join-Path $binary.DirectoryName $import)
        if ($import -match $RuntimePattern) {
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
