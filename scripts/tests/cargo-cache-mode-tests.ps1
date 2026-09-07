$ErrorActionPreference = 'Stop'

# Load the actual cache initializer without entering a Visual Studio shell or
# launching Cargo, so the command policy can also be exercised on Unix hosts.
$wrapper = Join-Path $PSScriptRoot '../cargo-cache-build.ps1'
$tokens = $null
$parseErrors = $null
$syntax = [System.Management.Automation.Language.Parser]::ParseFile(
    $wrapper, [ref] $tokens, [ref] $parseErrors)
if ($parseErrors.Count -gt 0) {
    throw ($parseErrors | Out-String)
}
$initializer = $syntax.Find({
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq 'Initialize-HypercolorCargoCache'
}, $false)
if ($null -eq $initializer) {
    throw 'Cargo cache initializer was not found'
}
Invoke-Expression $initializer.Extent.Text

function Get-Command {
    param([string] $Name, [string] $ErrorAction)
    if ($Name -ne 'sccache.exe') {
        throw "Unexpected tool lookup: $Name"
    }
    [pscustomobject] @{ Source = 'sccache.exe' }
}

$fixtureRoot = Join-Path ([System.IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString())
$RepoRoot = $fixtureRoot
$policyVariables = @(
    'CI', 'CARGO_INCREMENTAL', 'RUSTC_WRAPPER', 'HYPERCOLOR_FORCE_SCCACHE',
    'HYPERCOLOR_NO_SCCACHE', 'HYPERCOLOR_ITERATE', 'HYPERCOLOR_CACHE_DIR',
    'HYPERCOLOR_NO_FAST_LINK', 'CARGO_TARGET_DIR', 'MOZBUILD_STATE_PATH',
    'SCCACHE_DIR', 'SCCACHE_CACHE_SIZE', 'CMAKE_TOOLCHAIN_FILE',
    'CMAKE_C_COMPILER_LAUNCHER', 'CMAKE_CXX_COMPILER_LAUNCHER', 'CC', 'CXX',
    'CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER'
)
$originalEnvironment = @{}
foreach ($name in $policyVariables) {
    $originalEnvironment[$name] = [Environment]::GetEnvironmentVariable($name)
}

function Assert-Mode {
    param([string[]] $CargoArgs, [hashtable] $Environment, [string] $Incremental, [bool] $Cached)
    foreach ($name in $policyVariables) {
        [Environment]::SetEnvironmentVariable($name, $null)
    }
    $env:HYPERCOLOR_CACHE_DIR = Join-Path $fixtureRoot 'cache'
    $env:CMAKE_TOOLCHAIN_FILE = Join-Path $fixtureRoot 'toolchain.cmake'
    foreach ($entry in $Environment.GetEnumerator()) {
        [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value)
    }
    $script:CommandArgs = @('cargo') + $CargoArgs
    Initialize-HypercolorCargoCache | Out-Null
    if ($env:CARGO_INCREMENTAL -ne $Incremental) {
        throw "Wrong incremental mode for $CargoArgs : $env:CARGO_INCREMENTAL"
    }
    $expectedWrapper = if ($Cached) { 'sccache.exe' } else { '' }
    if ([string] $env:RUSTC_WRAPPER -ne $expectedWrapper) {
        throw "Wrong compiler wrapper for $CargoArgs : $env:RUSTC_WRAPPER"
    }
}

try {
    foreach ($command in @('build', 'test', 'run', 'check', 'clippy')) {
        Assert-Mode @($command) @{} '1' $false
    }
    Assert-Mode @('nextest', 'run') @{} '1' $false
    Assert-Mode @('build') @{ CI = 'true' } '0' $true
    Assert-Mode @('test') @{ CI = '1' } '0' $true
    Assert-Mode @('nextest', 'run') @{ CARGO_INCREMENTAL = '0' } '0' $true
    Assert-Mode @('check') @{ CARGO_INCREMENTAL = '0' } '0' $false
    Assert-Mode @('build') @{ HYPERCOLOR_FORCE_SCCACHE = '1' } '0' $true
    Assert-Mode @('build', '--release') @{} '0' $true
    Assert-Mode @('build') @{ CI = 'true'; HYPERCOLOR_ITERATE = '1' } '1' $false
    Assert-Mode @('test') @{ CARGO_INCREMENTAL = '1'; HYPERCOLOR_FORCE_SCCACHE = '1' } '1' $false
    Assert-Mode @('build') @{ RUSTC_WRAPPER = 'sccache.exe' } '1' $false
    Write-Host 'Cargo cache mode tests: PASS'
} finally {
    foreach ($name in $policyVariables) {
        [Environment]::SetEnvironmentVariable($name, $originalEnvironment[$name])
    }
    if (Test-Path $fixtureRoot) {
        Remove-Item -Recurse -Force $fixtureRoot
    }
}
