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
    'CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER', 'RUNNER_OS',
    'HYPERCOLOR_TEST_ARGUMENT_CAPTURE'
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

function Assert-WorkflowArguments {
    # Execute the real workflow's PowerShell command text at a script boundary.
    # Bare -- is consumed by PowerShell before the wrapper can inspect $args.
    $capture = $syntax.Find({
        param($node)
        $node -is [System.Management.Automation.Language.AssignmentStatementAst] -and
            $node.Left.Extent.Text -eq '$CommandArgs'
    }, $false)
    if ($null -eq $capture) {
        throw 'Cargo argument capture was not found'
    }
    $fixtureScripts = Join-Path $fixtureRoot 'scripts'
    New-Item -ItemType Directory -Force $fixtureScripts | Out-Null
    $captureScript = $capture.Extent.Text + @'

ConvertTo-Json -Compress -InputObject $CommandArgs |
    Add-Content -LiteralPath $env:HYPERCOLOR_TEST_ARGUMENT_CAPTURE
exit 0
'@
    Set-Content -LiteralPath (Join-Path $fixtureScripts 'cargo-cache-build.ps1') $captureScript
    $env:HYPERCOLOR_TEST_ARGUMENT_CAPTURE = Join-Path $fixtureRoot 'arguments.jsonl'
    $env:RUNNER_OS = 'Windows'

    $workflow = Get-Content (Join-Path $PSScriptRoot '../../.github/workflows/ci.yml')
    $commands = @()
    for ($i = 0; $i -lt $workflow.Count; $i += 1) {
        if ($workflow[$i] -notmatch '^        run: (.+)$') { continue }
        $value = $Matches[1]
        if ($value -match '^[>|]-?$') {
            $separator = if ($value.StartsWith('>')) { ' ' } else { "`n" }
            $body = @()
            while (($i + 1) -lt $workflow.Count -and
                ($workflow[$i + 1] -match '^          ' -or $workflow[$i + 1] -eq '')) {
                $i += 1
                $body += $workflow[$i] -replace '^          ', ''
            }
            $value = $body -join $separator
        }
        if ($value.Contains('./scripts/cargo-cache-build.ps1')) {
            $commands += $value.Replace('${{ env.RUST_WINDOWS_WORKSPACE_ARGS }}',
                '--workspace --exclude hypercolor-daemon --exclude hypercolor-app --exclude hypercolor-cli')
        }
    }
    $previousLocation = Get-Location
    try {
        Set-Location $fixtureRoot
        foreach ($command in $commands) {
            & ([scriptblock]::Create($command))
        }
    } finally {
        Set-Location $previousLocation
    }

    $invocations = @(Get-Content $env:HYPERCOLOR_TEST_ARGUMENT_CAPTURE |
        ForEach-Object { ,(ConvertFrom-Json $_) })
    if ($invocations.Count -ne 12) {
        throw "Expected 12 Windows Cargo invocations, captured $($invocations.Count)"
    }
    foreach ($invocation in $invocations) {
        if ($invocation[0] -ne 'cargo') { throw 'Lost the Cargo executable argument' }
        switch ($invocation[1]) {
            'clippy' {
                if (($invocation[-3..-1] -join ' ') -ne '-- -D warnings') {
                    throw "Clippy lost its compiler argument separator: $invocation"
                }
            }
            'test' {
                if (($invocation[-2..-1] -join ' ') -ne '-- --test-threads=1') {
                    throw "Allocation test lost its harness arguments: $invocation"
                }
            }
            'nextest' {
                $filterIndex = [array]::IndexOf($invocation, '-E')
                if ($invocation -contains 'windows-capture-fixtures' -and
                    ($filterIndex -lt 0 -or $invocation[$filterIndex + 1] -ne
                        'test(screen::windows) | binary(windows_host_input_fixture_tests) | binary(windows_capture_fixture_tests)')) {
                    throw "Nextest filter was split into separate arguments: $invocation"
                }
            }
        }
    }
    Write-Host 'Windows workflow argument tests: PASS'
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
    Assert-WorkflowArguments
} finally {
    foreach ($name in $policyVariables) {
        [Environment]::SetEnvironmentVariable($name, $originalEnvironment[$name])
    }
    if (Test-Path $fixtureRoot) {
        Remove-Item -Recurse -Force $fixtureRoot
    }
}
