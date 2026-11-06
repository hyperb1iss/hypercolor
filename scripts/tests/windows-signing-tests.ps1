$ErrorActionPreference = 'Stop'

# Exercises the parts of sign-windows-artifacts.ps1 that decide what gets
# signed and how the uninstaller hook is spelled, on any OS. Signing itself
# needs Windows and an Azure account, which the Windows Signing Selftest
# workflow covers.
$script = Join-Path $PSScriptRoot '../sign-windows-artifacts.ps1'
$pwsh = (Get-Command pwsh -ErrorAction Stop).Source
$configNames = @(
    'ARTIFACT_SIGNING_ACCOUNT', 'ARTIFACT_SIGNING_ENDPOINT', 'ARTIFACT_SIGNING_CERTIFICATE_PROFILE',
    'AZURE_CLIENT_ID', 'AZURE_TENANT_ID', 'AZURE_SUBSCRIPTION_ID', 'WINDOWS_SIGNING_PUBLISHER'
)
$complete = @{
    ARTIFACT_SIGNING_ACCOUNT = 'hypercolor'
    ARTIFACT_SIGNING_ENDPOINT = 'https://eus.codesigning.azure.net/'
    ARTIFACT_SIGNING_CERTIFICATE_PROFILE = 'hypercolor-public'
    AZURE_CLIENT_ID = '00000000-0000-0000-0000-000000000001'
    AZURE_TENANT_ID = '00000000-0000-0000-0000-000000000002'
    AZURE_SUBSCRIPTION_ID = '00000000-0000-0000-0000-000000000003'
    WINDOWS_SIGNING_PUBLISHER = 'Fixture Publisher'
}
$failures = 0

function Invoke-Resolve {
    param([hashtable] $Environment, [string] $Ref, [string] $Mode = '')

    $output = [IO.Path]::GetTempFileName()
    $saved = @{}
    $names = $configNames + @('GITHUB_REF', 'GITHUB_OUTPUT', 'GITHUB_ACTIONS', 'RELEASE_ARTIFACTS', 'SIGNING_REF')
    foreach ($name in $names) {
        $saved[$name] = [Environment]::GetEnvironmentVariable($name)
        [Environment]::SetEnvironmentVariable($name, $null)
    }
    try {
        foreach ($entry in $Environment.GetEnumerator()) {
            [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value)
        }
        $env:GITHUB_REF = $Ref
        $env:GITHUB_OUTPUT = $output
        $env:RELEASE_ARTIFACTS = $Mode
        $log = & $pwsh -NoProfile -NonInteractive -File $script resolve 2>&1 | Out-String
        [pscustomobject] @{
            ExitCode = $LASTEXITCODE
            Output = (Get-Content -Raw -LiteralPath $output)
            Log = $log
        }
    } finally {
        foreach ($name in $names) {
            [Environment]::SetEnvironmentVariable($name, $saved[$name])
        }
        Remove-Item -LiteralPath $output -Force
    }
}

function Assert-Resolve {
    param([string] $Name, [hashtable] $Environment, [string] $Ref, [string] $Mode, [string] $Enabled, [string] $LogPattern)

    $result = Invoke-Resolve -Environment $Environment -Ref $Ref -Mode $Mode
    $problems = @()
    if ($Enabled -eq 'error') {
        if ($result.ExitCode -eq 0) { $problems += 'expected a failure' }
        if ($result.Output) { $problems += "wrote an output on failure: $($result.Output)" }
    } else {
        if ($result.ExitCode -ne 0) { $problems += "exited $($result.ExitCode)" }
        if ("$($result.Output)".Trim() -ne "enabled=$Enabled") { $problems += "wrote '$($result.Output)'" }
    }
    if ($LogPattern -and $result.Log -notmatch $LogPattern) { $problems += "log lacks /$LogPattern/" }
    if ($problems.Count -gt 0) {
        Write-Host "FAIL $Name : $($problems -join '; ')`n$($result.Log)"
        $script:failures++
    } else {
        Write-Host "ok   $Name"
    }
}

$partial = @{ ARTIFACT_SIGNING_ACCOUNT = 'hypercolor'; AZURE_CLIENT_ID = $complete.AZURE_CLIENT_ID }
$badEndpoint = $complete.Clone()
$badEndpoint.ARTIFACT_SIGNING_ENDPOINT = 'https://eus.codesigning.example.com'
$badGuid = $complete.Clone()
$badGuid.AZURE_TENANT_ID = 'not-a-guid'
$switchOff = $complete.Clone()
$switchOff.Remove('ARTIFACT_SIGNING_ACCOUNT')

Assert-Resolve 'unconfigured tag builds unsigned' @{} 'refs/tags/v1.0.0' '' 'false' 'not configured'
Assert-Resolve 'configured tag signs' $complete 'refs/tags/v1.0.0' '' 'true' 'Fixture Publisher'
Assert-Resolve 'configured full dispatch on main signs' $complete 'refs/heads/main' 'full' 'true' ''
Assert-Resolve 'warm never signs' $complete 'refs/heads/main' 'warm' 'false' 'warm'
Assert-Resolve 'feature branch never signs' $complete 'refs/heads/nova/windows-signing' 'full' 'false' 'only for main and v\* tags'
Assert-Resolve 'non-release tag never signs' $complete 'refs/tags/native-v0.4.0' '' 'false' 'only for main'
Assert-Resolve 'empty switch builds unsigned' $switchOff 'refs/tags/v1.0.0' '' 'false' 'ARTIFACT_SIGNING_ACCOUNT is empty'
Assert-Resolve 'partial configuration fails' $partial 'refs/tags/v1.0.0' '' 'error' 'ARTIFACT_SIGNING_ENDPOINT, ARTIFACT_SIGNING_CERTIFICATE_PROFILE, AZURE_TENANT_ID'
Assert-Resolve 'partial configuration on warm stays quiet' $partial 'refs/heads/main' 'warm' 'false' ''
Assert-Resolve 'endpoint outside Artifact Signing fails' $badEndpoint 'refs/tags/v1.0.0' '' 'error' 'regional endpoint'
Assert-Resolve 'non-GUID tenant fails' $badGuid 'refs/tags/v1.0.0' '' 'error' 'AZURE_TENANT_ID must be a GUID'

$releaseCheck = $complete.Clone()
$releaseCheck.SIGNING_REF = 'refs/tags/v1.0.0'
Assert-Resolve 'SIGNING_REF lets the Release workflow ask about its tag' $releaseCheck 'refs/heads/main' '' 'true' ''

# Load every constant and function without running the command switch.
$tokens = $null
$parseErrors = $null
$syntax = [System.Management.Automation.Language.Parser]::ParseFile(
    (Resolve-Path $script).Path, [ref] $tokens, [ref] $parseErrors)
if ($parseErrors.Count -gt 0) {
    throw ($parseErrors | Out-String)
}
foreach ($statement in $syntax.EndBlock.Statements) {
    if ($statement -isnot [System.Management.Automation.Language.SwitchStatementAst]) {
        Invoke-Expression $statement.Extent.Text
    }
}

function Assert-True {
    param([string] $Name, [bool] $Condition)

    if ($Condition) {
        Write-Host "ok   $Name"
    } else {
        Write-Host "FAIL $Name"
        $script:failures++
    }
}

$signTokens = @(Get-SignCommandTokens 'D:\a\hypercolor\hypercolor\scripts\sign-windows-artifacts.ps1')
Assert-True 'sign command ends with the sign verb and the %1 placeholder Tauri substitutes' (
    ($signTokens[-2] -eq 'sign') -and ($signTokens[-1] -eq '%1'))
Assert-True 'sign command spells the script path with forward slashes' (
    $signTokens -contains 'D:/a/hypercolor/hypercolor/scripts/sign-windows-artifacts.ps1')
Assert-True 'uninstaller hook line matches Rust Debug formatting' (
    (Format-SignCommandLine @('pwsh', '-File', 'D:/x.ps1', 'sign', '%1')) -eq '"pwsh" "-File" "D:/x.ps1" "sign" "%1"')
foreach ($bad in 'D:\x.ps1', 'say "hi"', "it's") {
    $threw = $false
    try { Format-SignCommandLine @('pwsh', $bad) | Out-Null } catch { $threw = $true }
    Assert-True "uninstaller hook line refuses $bad" $threw
}

$separator = [IO.Path]::DirectorySeparatorChar
$pluginDir = @('', 'work', 'release', 'nsis', 'x64', 'Plugins', 'x86-unicode') -join $separator
Assert-True 'a stock plugin in the Tauri copy triggers the extra plugins' (
    Test-TauriPluginCopy (Join-Path $pluginDir 'System.dll'))
Assert-True 'Tauri additional plugins do not' (
    -not (Test-TauriPluginCopy (Join-Path (Join-Path $pluginDir 'additional') 'nsis_tauri_utils.dll')))
Assert-True 'the uninstaller temp file does not' (
    -not (Test-TauriPluginCopy (Join-Path ([IO.Path]::GetTempPath()) 'nst5E1F.tmp')))
Assert-True 'bundled app binaries do not' (
    -not (Test-TauriPluginCopy (@('', 'work', 'target', 'bundle-stage', 'dlls', 'libEGL.dll') -join $separator)))

# Drive sign and verify against fake signatures. Each fixture file's text
# is its signature: "signed|<signer>|<status>|ts" or anything else for none.
function Write-Failure { param([string] $Message) throw "FAILURE: $Message" }
function Assert-Windows {}
function Get-AuthenticodeSignature {
    param([string] $LiteralPath)

    $parts = (Get-Content -Raw -LiteralPath $LiteralPath).Trim() -split '\|'
    if ($parts[0] -ne 'signed') {
        return [pscustomobject] @{
            Status = 'NotSigned'; StatusMessage = 'unsigned'; SignerCertificate = $null; TimeStamperCertificate = $null
        }
    }
    $certificate = [pscustomobject] @{ Name = $parts[1] }
    $certificate | Add-Member -MemberType ScriptMethod -Name GetNameInfo -Value { param($type, $issuer) $this.Name }
    [pscustomobject] @{
        Status = $parts[2]
        StatusMessage = $parts[2]
        SignerCertificate = $certificate
        TimeStamperCertificate = if ($parts[3] -eq 'ts') { 'timestamp' } else { $null }
    }
}
$fixtureInstallers = @{}
function Expand-Installer { param([string] $File, [string] $Name) $script:fixtureInstallers[$File] }
$ours = 'signed|Fixture Publisher|Valid|ts'
function Invoke-SignTool { param([string] $File) Set-Content -LiteralPath $File -Value $script:ours -NoNewline }

$fixtureRoot = Join-Path ([IO.Path]::GetTempPath()) "windows-signing-tests-$PID"
New-Item -ItemType Directory -Force -Path $fixtureRoot | Out-Null
$env:HYPERCOLOR_SIGNING_PUBLISHER = 'Fixture Publisher'
$env:HYPERCOLOR_SIGNING_LOG = Join-Path $fixtureRoot 'signed-files.tsv'
foreach ($name in 'HYPERCOLOR_SIGNTOOL', 'HYPERCOLOR_SIGNING_DLIB', 'HYPERCOLOR_SIGNING_METADATA') {
    [Environment]::SetEnvironmentVariable($name, 'fixture')
}

function New-FixtureTree {
    param([string] $Name, [hashtable] $Files)

    $root = Join-Path $fixtureRoot $Name
    foreach ($entry in $Files.GetEnumerator()) {
        $path = Join-Path $root $entry.Key
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $path) | Out-Null
        Set-Content -LiteralPath $path -Value $entry.Value -NoNewline
    }
    $root
}

try {
    $vendor = 'signed|Microsoft Corporation|Valid|ts'
    $pawnIo = 'signed|namazso.eu|NotTrusted|ts'
    $pawnIoHash = [Convert]::ToHexString(
        [Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($pawnIo)))
    $payload = @{
        'hypercolor-app.exe' = $ours; 'hypercolor-daemon.exe' = $ours; 'hypercolor.exe' = $ours
        'libEGL.dll' = $ours; 'libGLESv2.dll' = $ours; 'uninstall.exe' = $ours
        'msvcp140.dll' = $vendor; 'vcruntime140.dll' = $vendor; 'vcruntime140_1.dll' = $vendor
        'tools\hypercolor-smbus-service.exe' = $ours; 'tools\hypercolor-windows-helper.exe' = $ours
        'tools\vcruntime140.dll' = $vendor
        'tools\pawnio\PawnIO_setup.exe' = $pawnIo
        'tools\pawnio\manifest.json' = "{`"pawnio_setup`":{`"sha256`":`"$pawnIoHash`"}}"
        '$PLUGINSDIR\System.dll' = $ours; '$PLUGINSDIR\nsExec.dll' = $ours
        'ui\index.html' = '<html></html>'
        '[NSIS].nsi' = 'signed build'
    }

    function Test-Verify {
        param([string] $Name, [hashtable] $Signed, [hashtable] $Unsigned, [string] $Expect, [string] $InstallerSignature = $script:ours)

        $script:Installer = Join-Path $fixtureRoot "$Name-setup.exe"
        Set-Content -LiteralPath $script:Installer -Value $InstallerSignature -NoNewline
        $script:UnsignedInstaller = Join-Path $fixtureRoot "$Name-unsigned-setup.exe"
        Set-Content -LiteralPath $script:UnsignedInstaller -Value 'unsigned' -NoNewline
        $script:fixtureInstallers[$script:Installer] = New-FixtureTree "$Name-signed" $Signed
        $script:fixtureInstallers[$script:UnsignedInstaller] = New-FixtureTree "$Name-unsigned" $Unsigned
        # Collect while streaming: a throw would drop a captured assignment.
        $lines = [Collections.Generic.List[string]]::new()
        $threw = $false
        try {
            Invoke-Verify 6>&1 | ForEach-Object { $lines.Add("$_") }
        } catch {
            $threw = $true
            $lines.Add($_.Exception.Message)
        }
        $log = $lines -join "`n"
        if ($Expect) {
            Assert-True "verify catches $Name" ($threw -and $log.Contains($Expect))
        } else {
            Assert-True "verify passes $Name" (-not $threw)
        }
        if (($Expect -and -not $log.Contains($Expect)) -or (-not $Expect -and $threw)) {
            Write-Host $log
        }
    }

    $unsigned = $payload.Clone()
    foreach ($key in @($unsigned.Keys)) {
        if ($unsigned[$key] -eq $ours) { $unsigned[$key] = 'unsigned' }
    }
    $unsigned['[NSIS].nsi'] = 'unsigned build'

    Test-Verify 'a fully signed installer' $payload $unsigned ''
    $case = $payload.Clone(); $case['$PLUGINSDIR\nsExec.dll'] = 'unsigned'
    Test-Verify 'an unsigned NSIS plugin' $case $unsigned '$PLUGINSDIR\nsExec.dll: status NotSigned'
    $case = $payload.Clone(); $case['tools\pawnio\PawnIO_setup.exe'] = $ours
    Test-Verify 'a re-signed vendor binary' $case $unsigned 'PawnIO_setup.exe is a vendor binary but carries the Hypercolor signature'
    Test-Verify 'a changed PawnIO setup' $case $unsigned 'differs from its pin'
    $case = $payload.Clone(); $case.Remove('uninstall.exe')
    $twin = $unsigned.Clone(); $twin.Remove('uninstall.exe')
    Test-Verify 'a missing uninstaller' $case $twin 'uninstall.exe is missing from the installer'
    $case = $payload.Clone(); $case['tools\hypercolor-windows-helper.exe'] = 'signed|Someone Else|Valid|ts'
    Test-Verify 'a foreign first-party signature' $case $unsigned "signed by 'Someone Else'"
    $case = $payload.Clone(); $case['hypercolor-daemon.exe'] = 'signed|Fixture Publisher|Valid|'
    Test-Verify 'an untimestamped binary' $case $unsigned 'hypercolor-daemon.exe: no timestamp countersignature'
    Test-Verify 'an untimestamped installer' $payload $unsigned 'no timestamp countersignature' 'signed|Fixture Publisher|Valid|'
    $case = $payload.Clone(); $case['ui\index.html'] = '<html>changed</html>'
    Test-Verify 'a payload file the rebundle changed' $case $unsigned 'ui\index.html differs between'
    $case = $payload.Clone(); $case['ui\extra.js'] = 'x'
    Test-Verify 'a file only the signed installer ships' $case $unsigned 'ui\extra.js is in the signed installer but not the unsigned one'
    $case = $payload.Clone(); $case['vcruntime140.dll'] = 'signed|Microsoft Corporation|HashMismatch|ts'
    Test-Verify 'a VC++ runtime whose hash no longer matches' $case $unsigned "vcruntime140.dll is a VC++ runtime but carries a HashMismatch signature"
    $case = $payload.Clone(); $case['msvcp140.dll'] = 'signed|Unexpected Publisher|Valid|ts'
    Test-Verify 'a VC++ runtime from an unexpected publisher' $case $unsigned "msvcp140.dll is a VC++ runtime but carries a Valid signature from 'Unexpected Publisher'"
    $case = $payload.Clone(); $case['tools\vcruntime140.dll'] = 'signed|Microsoft Corporation|Valid|ts|another build'
    Test-Verify 'a substituted but validly signed VC++ runtime' $case $unsigned 'tools\vcruntime140.dll differs between'
    $case = $payload.Clone(); $case['tools\pawnio\PawnIO_setup.exe'] = 'signed|namazso.eu|Valid|ts|another build'
    Test-Verify 'a substituted PawnIO setup' $case $unsigned 'tools\pawnio\PawnIO_setup.exe differs between'

    # Sign: first-party files get signed, vendor signatures stay untouched,
    # and the Tauri plugin copy picks up the extra plugins.
    $tree = New-FixtureTree 'sign' @{
        'app.exe' = 'unsigned'
        'stale.exe' = 'signed|Fixture Publisher|HashMismatch|ts'
        'PawnIO_setup.exe' = $pawnIo
        'nsis\x64\Plugins\x86-unicode\System.dll' = 'unsigned'
        'nsis\x64\Plugins\x86-unicode\nsExec.dll' = 'unsigned'
        'nsis\x64\Plugins\x86-unicode\Banner.dll' = 'unsigned'
    }
    $read = { param($relative) (Get-Content -Raw -LiteralPath (Join-Path $tree $relative)).Trim() }
    Invoke-Sign (Join-Path $tree 'app.exe') 6>$null
    Assert-True 'sign signs an unsigned first-party binary' ((& $read 'app.exe') -eq $ours)
    Invoke-Sign (Join-Path $tree 'stale.exe') 6>$null
    Assert-True 'sign re-signs a binary whose Hypercolor signature went stale' ((& $read 'stale.exe') -eq $ours)
    Invoke-Sign (Join-Path $tree 'PawnIO_setup.exe') 6>$null
    Assert-True 'sign leaves a vendor-signed binary byte for byte' ((& $read 'PawnIO_setup.exe') -eq $pawnIo)
    Invoke-Sign (Join-Path $tree 'nsis\x64\Plugins\x86-unicode\System.dll') 6>$null
    Assert-True 'sign covers nsExec.dll beside a Tauri plugin copy' (
        (& $read 'nsis\x64\Plugins\x86-unicode\nsExec.dll') -eq $ours)
    Assert-True 'sign leaves plugins the installer never loads alone' (
        (& $read 'nsis\x64\Plugins\x86-unicode\Banner.dll') -eq 'unsigned')
    $logged = @(Get-Content -LiteralPath $env:HYPERCOLOR_SIGNING_LOG)
    Assert-True 'sign logs exactly the four files it signed' ($logged.Count -eq 4)
} finally {
    Remove-Item -LiteralPath $fixtureRoot -Recurse -Force -ErrorAction SilentlyContinue
}

if ($failures -gt 0) {
    throw "$failures Windows signing check(s) failed"
}
Write-Host 'Windows signing policy checks passed'
