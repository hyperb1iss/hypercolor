<#
Authenticode signing for the Windows release, through Azure Artifact Signing.

  resolve   Decide whether this run signs, from the repository variables,
            and write enabled=true|false to GITHUB_OUTPUT. Runs on any OS.
  setup     Fetch the pinned Artifact Signing dlib, find signtool, write the
            dlib metadata and the Tauri signCommand config, and cache a code
            signing token while the OIDC login is still fresh.
  sign-payload  Sign every first-party program a unit or the stage holds, so a
            unit can be produced from their final bytes.
  sign      Sign one file. Tauri's signCommand runs this for every binary it
            bundles, and makensis runs it on the uninstaller mid-build.
  verify    Prove an installer ships only Hypercolor-signed first-party
            binaries and untouched vendor binaries.
  selftest  Sign a probe binary directly and through the uninstaller hook.

docs/development/RELEASING.md covers the one-time Azure setup.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [ValidateSet('resolve', 'setup', 'sign', 'sign-payload', 'verify', 'selftest')]
    [string] $Command,

    # The file to sign (sign) or the probe binary (selftest).
    [Parameter(Position = 1)]
    [string] $Path = '',

    # The signed installer to verify, and optionally the unsigned build of
    # the same inputs that the release lane tested.
    [string] $Installer = '',
    [string] $UnsignedInstaller = ''
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# The signtool dlib. Bump the version and hash together.
$DlibPackage = 'microsoft.artifactsigning.client'
$DlibVersion = '1.0.146'
$DlibSha256 = '7761E5D8F31E54B764DA21D33CD8B0A2940E1B05526C7DFCB5E94B0431426F70'
# Microsoft's dlib docs name SignTool from SDK 10.0.22621 or newer.
$MinimumSdkVersion = [version] '10.0.22621.0'
# Artifact Signing certificates live for 72 hours, so every signature needs
# a countersigned timestamp to stay valid.
$TimestampUrl = 'http://timestamp.acs.microsoft.com'
$CodeSigningScope = 'https://codesigning.azure.net/.default'
$SignatureDescription = 'Hypercolor'
$SignatureUrl = 'https://github.com/hyperb1iss/hypercolor'

# ARTIFACT_SIGNING_ACCOUNT is the switch: empty means unsigned, and once it
# is set every other name must be too.
$SwitchName = 'ARTIFACT_SIGNING_ACCOUNT'
$ConfigNames = @(
    'ARTIFACT_SIGNING_ACCOUNT',
    'ARTIFACT_SIGNING_ENDPOINT',
    'ARTIFACT_SIGNING_CERTIFICATE_PROFILE',
    'AZURE_CLIENT_ID',
    'AZURE_TENANT_ID',
    'AZURE_SUBSCRIPTION_ID',
    'WINDOWS_SIGNING_PUBLISHER'
)

# The dlib authenticates through DefaultAzureCredential. CI signs only as
# the identity azure/login put in the Azure CLI, so skip every other probe.
$ExcludedCredentials = @(
    'ManagedIdentityCredential',
    'WorkloadIdentityCredential',
    'SharedTokenCacheCredential',
    'VisualStudioCredential',
    'VisualStudioCodeCredential',
    'AzurePowerShellCredential',
    'AzureDeveloperCliCredential',
    'InteractiveBrowserCredential'
)

# Vendor binaries the installer carries, never re-signed. The VC++ runtime
# must arrive with a valid Microsoft signature. PawnIO's setup may chain to
# a root the runner does not trust, so it is held to the SHA-256 pin that
# install-bundled-pawnio.ps1 also checks at install time.
$MicrosoftRuntimeBinaries = @('msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll')
$MicrosoftSigner = 'Microsoft Corporation'
$ThirdPartyBinaries = @('PawnIO_setup.exe') + $MicrosoftRuntimeBinaries

# Where 7-Zip extracts the unit an installer carries.
$PayloadUnit = '$PLUGINSDIR\unit'

# Programs a build writes to its release directory, each signed in place
# before the unit is produced: a unit holds their exact bytes.
$ReleasePrograms = @(
    'hypercolor-app.exe',
    'hypercolor-daemon.exe',
    'hypercolor.exe',
    'hypercolor-smbus-service.exe',
    'hypercolor-update-bootstrap.exe',
    'hypercolor-windows-helper.exe'
)

# What the stage holds for itself, unit members and the stable launcher alike.
$StagedPrograms = @(
    'dlls\libEGL.dll',
    'dlls\libGLESv2.dll',
    'tools\hypercolor-launcher.exe'
)

# Payload paths (as 7-Zip extracts them) that every installer must carry
# with a Hypercolor signature.
$RequiredSignedPayload = @(
    'Hypercolor.new.exe',
    "$PayloadUnit\hypercolor-app.exe",
    "$PayloadUnit\hypercolor-daemon.exe",
    "$PayloadUnit\hypercolor.exe",
    "$PayloadUnit\libEGL.dll",
    "$PayloadUnit\libGLESv2.dll",
    "$PayloadUnit\tools\hypercolor-smbus-service.exe",
    "$PayloadUnit\tools\hypercolor-update-bootstrap.exe",
    "$PayloadUnit\tools\hypercolor-windows-helper.exe",
    'uninstall.exe'
)

# Tauri signs its own copy of five stock NSIS plugins before makensis
# embeds them. The Hypercolor template and hooks also call these, so they
# are signed in the same copy when Tauri signs the first plugin there.
$ExtraNsisPlugins = @('nsExec.dll')

function Write-Failure {
    param([string] $Message)

    if ($env:GITHUB_ACTIONS -eq 'true') {
        Write-Host "::error title=Windows signing::$Message"
    }
    [Console]::Error.WriteLine($Message)
    exit 1
}

function Write-Notice {
    param([string] $Message)

    if ($env:GITHUB_ACTIONS -eq 'true') {
        Write-Host "::notice title=Windows signing::$Message"
    } else {
        Write-Host $Message
    }
}

function Assert-Windows {
    if (-not $IsWindows) {
        Write-Failure "'$Command' runs only on Windows"
    }
}

function Get-RequiredEnv {
    param([string] $Name)

    $value = [Environment]::GetEnvironmentVariable($Name)
    if ([string]::IsNullOrWhiteSpace($value)) {
        Write-Failure "$Name is not set; run 'sign-windows-artifacts.ps1 setup' first"
    }
    $value
}

function Add-GitHubFile {
    param([string] $Variable, [string] $Line)

    $file = [Environment]::GetEnvironmentVariable($Variable)
    if ($file) {
        Add-Content -LiteralPath $file -Value $Line -Encoding utf8
    }
}

function Get-SigningConfig {
    $config = [ordered] @{}
    foreach ($name in $ConfigNames) {
        $value = [Environment]::GetEnvironmentVariable($name)
        $config[$name] = if ($null -eq $value) { '' } else { $value.Trim() }
    }
    $config
}

function Get-ConfigProblems {
    param($Config)

    $problems = @()
    $missing = @($ConfigNames | Where-Object { -not $Config[$_] })
    if ($missing.Count -gt 0) {
        $problems += "missing repository variables: $($missing -join ', ')"
    }
    foreach ($name in 'AZURE_CLIENT_ID', 'AZURE_TENANT_ID', 'AZURE_SUBSCRIPTION_ID') {
        $parsed = [guid]::Empty
        if ($Config[$name] -and -not [guid]::TryParse($Config[$name], [ref] $parsed)) {
            $problems += "$name must be a GUID"
        }
    }
    $endpoint = $Config['ARTIFACT_SIGNING_ENDPOINT']
    if ($endpoint -and $endpoint -notmatch '^https://[a-z0-9]+\.codesigning\.azure\.net/?$') {
        $problems += "ARTIFACT_SIGNING_ENDPOINT must be the account's regional endpoint, like https://eus.codesigning.azure.net"
    }
    foreach ($name in 'ARTIFACT_SIGNING_ACCOUNT', 'ARTIFACT_SIGNING_CERTIFICATE_PROFILE') {
        if ($Config[$name] -and $Config[$name] -notmatch '^[A-Za-z0-9][A-Za-z0-9-]*$') {
            $problems += "$name may hold only letters, digits, and hyphens"
        }
    }
    if ($Config['WINDOWS_SIGNING_PUBLISHER'] -match '[\r\n"]') {
        $problems += 'WINDOWS_SIGNING_PUBLISHER must be the certificate common name on one line'
    }
    $problems
}

function Test-SigningRef {
    param([string] $Ref)

    # The windows-signing environment admits exactly these refs, and its
    # federated credential is the only way to a signing token.
    $Ref -eq 'refs/heads/main' -or $Ref -like 'refs/tags/v*'
}

function Invoke-Resolve {
    $config = Get-SigningConfig
    # SIGNING_REF lets the Release workflow ask about the tag it is about to
    # push; GitHub reserves GITHUB_REF itself.
    $ref = if ($env:SIGNING_REF) { $env:SIGNING_REF } else { "$env:GITHUB_REF" }
    $enabled = $false

    if (-not $config[$SwitchName]) {
        $stray = @($ConfigNames | Where-Object { $config[$_] })
        if ($stray.Count -gt 0) {
            Write-Notice "Windows signing is off because $SwitchName is empty, so this run builds an unsigned installer (also set: $($stray -join ', '))."
        } else {
            Write-Notice 'Windows signing is not configured, so this run builds an unsigned installer. docs/development/RELEASING.md lists the repository variables that turn it on.'
        }
    } elseif ($env:RELEASE_ARTIFACTS -eq 'warm') {
        Write-Notice 'A warm run only seeds the release compiler cache, so its Windows installer stays unsigned.'
    } elseif (-not (Test-SigningRef $ref)) {
        Write-Notice "Windows signing runs only for main and v* tags, so $ref builds an unsigned installer."
    } else {
        $problems = @(Get-ConfigProblems $config)
        if ($problems.Count -gt 0) {
            Write-Failure "Windows signing is switched on by $SwitchName but cannot run: $($problems -join '; ')"
        }
        $enabled = $true
        Write-Notice "Signing the Windows release as '$($config['WINDOWS_SIGNING_PUBLISHER'])' with Artifact Signing account $($config['ARTIFACT_SIGNING_ACCOUNT']), certificate profile $($config['ARTIFACT_SIGNING_CERTIFICATE_PROFILE'])."
    }

    $value = if ($enabled) { 'true' } else { 'false' }
    Add-GitHubFile 'GITHUB_OUTPUT' "enabled=$value"
    Write-Host "enabled=$value"
}

function Get-SignCommandTokens {
    param([string] $ScriptPath)

    # Tauri renders the uninstaller hook with Rust's Debug formatting, which
    # escapes backslashes and quotes, so the script path uses forward slashes
    # and Format-SignCommandLine can reproduce the line exactly.
    $script = $ScriptPath -replace '\\', '/'
    @('pwsh', '-NoLogo', '-NoProfile', '-NonInteractive', '-File', $script, 'sign', '%1')
}

function Format-SignCommandLine {
    param([string[]] $Tokens)

    # Debug formatting escapes quotes and backslashes, and installer.nsi
    # wraps the whole line in single quotes for !uninstfinalize.
    foreach ($token in $Tokens) {
        if ($token -match "[`"'\\]") {
            throw "sign command token would be mangled on its way to the uninstaller hook: $token"
        }
    }
    '"' + ($Tokens -join '" "') + '"'
}

function Find-SignTool {
    $kitsBin = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    $signtool = Get-ChildItem -LiteralPath $kitsBin -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '^\d+\.\d+\.\d+\.\d+$' -and [version] $_.Name -ge $MinimumSdkVersion } |
        Sort-Object { [version] $_.Name } -Descending |
        ForEach-Object { Join-Path $_.FullName 'x64\signtool.exe' } |
        Where-Object { Test-Path -LiteralPath $_ } |
        Select-Object -First 1
    if (-not $signtool) {
        Write-Failure "no x64 signtool.exe from Windows SDK $MinimumSdkVersion or newer under $kitsBin"
    }
    $signtool
}

function Invoke-Setup {
    Assert-Windows
    $config = Get-SigningConfig
    $problems = @(Get-ConfigProblems $config)
    if ($problems.Count -gt 0) {
        Write-Failure "cannot set up Windows signing: $($problems -join '; ')"
    }

    # GitHub's OIDC token expires about five minutes after azure/login uses
    # it, and the CLI cannot mint a code signing token once it has. Fetch
    # that token first so the CLI serves it from cache for the next hour.
    & az account get-access-token --scope $CodeSigningScope --output none
    if ($LASTEXITCODE -ne 0) {
        Write-Failure "the Azure CLI could not get a code signing token (exit $LASTEXITCODE); check the federated credential and azure/login"
    }

    $tempRoot = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
    $root = Join-Path $tempRoot 'artifact-signing'
    if (Test-Path -LiteralPath $root) {
        Remove-Item -LiteralPath $root -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $root | Out-Null

    $package = Join-Path $root "$DlibPackage.$DlibVersion.nupkg"
    $url = "https://api.nuget.org/v3-flatcontainer/$DlibPackage/$DlibVersion/$DlibPackage.$DlibVersion.nupkg"
    Invoke-WebRequest -Uri $url -OutFile $package
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $package).Hash.ToUpperInvariant()
    if ($actual -ne $DlibSha256) {
        Write-Failure "SHA256 mismatch for $url; expected $DlibSha256, got $actual"
    }
    $client = Join-Path $root 'client'
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::ExtractToDirectory($package, $client)
    $dlib = Join-Path $client 'bin\x64\Azure.CodeSigning.Dlib.dll'
    if (-not (Test-Path -LiteralPath $dlib)) {
        Write-Failure "the $DlibPackage $DlibVersion package has no bin\x64\Azure.CodeSigning.Dlib.dll"
    }
    $signtool = Find-SignTool

    $metadata = Join-Path $root 'metadata.json'
    [ordered] @{
        Endpoint = $config['ARTIFACT_SIGNING_ENDPOINT'].TrimEnd('/')
        CodeSigningAccountName = $config['ARTIFACT_SIGNING_ACCOUNT']
        CertificateProfileName = $config['ARTIFACT_SIGNING_CERTIFICATE_PROFILE']
        CorrelationId = "$env:GITHUB_REPOSITORY/$env:GITHUB_RUN_ID/$env:GITHUB_RUN_ATTEMPT"
        ExcludeCredentials = $ExcludedCredentials
    } | ConvertTo-Json | Set-Content -LiteralPath $metadata -Encoding utf8NoBOM

    $tokens = @(Get-SignCommandTokens $PSCommandPath)
    $null = Format-SignCommandLine $tokens
    $tauriConfig = Join-Path $root 'tauri.windows.sign.conf.json'
    @{
        bundle = @{
            windows = @{
                signCommand = @{ cmd = $tokens[0]; args = @($tokens | Select-Object -Skip 1) }
            }
        }
    } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $tauriConfig -Encoding utf8NoBOM

    $log = Join-Path $root 'signed-files.tsv'
    New-Item -ItemType File -Force -Path $log | Out-Null
    $exports = [ordered] @{
        HYPERCOLOR_SIGNTOOL = $signtool
        HYPERCOLOR_SIGNING_DLIB = $dlib
        HYPERCOLOR_SIGNING_METADATA = $metadata
        HYPERCOLOR_SIGNING_PUBLISHER = $config['WINDOWS_SIGNING_PUBLISHER']
        HYPERCOLOR_SIGNING_LOG = $log
        HYPERCOLOR_TAURI_SIGN_CONFIG = $tauriConfig
    }
    foreach ($entry in $exports.GetEnumerator()) {
        Add-GitHubFile 'GITHUB_ENV' "$($entry.Key)=$($entry.Value)"
        [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value)
    }
    Write-Host "signtool: $signtool"
    Write-Host "dlib: $DlibPackage $DlibVersion"
    Write-Host "Tauri sign config: $tauriConfig"
}

function Get-SignerName {
    param($Signature)

    if ($null -eq $Signature -or $null -eq $Signature.SignerCertificate) {
        return ''
    }
    $Signature.SignerCertificate.GetNameInfo(
        [Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false)
}

function Get-SignatureProblem {
    param([string] $File, [string] $Publisher)

    $signature = Get-AuthenticodeSignature -LiteralPath $File
    $signer = Get-SignerName $signature
    if ($signature.Status -ne 'Valid') {
        return "status $($signature.Status) ($($signature.StatusMessage))"
    }
    if ($signer -ne $Publisher) {
        return "signed by '$signer', expected '$Publisher'"
    }
    if ($null -eq $signature.TimeStamperCertificate) {
        return 'no timestamp countersignature'
    }
    ''
}

function Invoke-SignTool {
    param([string] $File)

    $arguments = @(
        'sign', '/v',
        '/fd', 'SHA256',
        '/tr', $TimestampUrl, '/td', 'SHA256',
        '/d', $SignatureDescription, '/du', $SignatureUrl,
        '/dlib', (Get-RequiredEnv 'HYPERCOLOR_SIGNING_DLIB'),
        '/dmdf', (Get-RequiredEnv 'HYPERCOLOR_SIGNING_METADATA'),
        $File
    )
    $signtool = Get-RequiredEnv 'HYPERCOLOR_SIGNTOOL'
    # The signing service and the timestamp authority are remote; give a
    # transient failure two more tries, then fail with every attempt's log.
    $delays = @(10, 30)
    for ($attempt = 1; ; $attempt++) {
        $output = & $signtool @arguments 2>&1 | Out-String
        if ($LASTEXITCODE -eq 0) {
            Write-Host $output.TrimEnd()
            return
        }
        Write-Host "signtool attempt $attempt for $File exited $LASTEXITCODE`n$($output.TrimEnd())"
        if ($attempt -gt $delays.Count) {
            Write-Failure "signtool could not sign $File after $attempt attempts"
        }
        Start-Sleep -Seconds $delays[$attempt - 1]
    }
}

function Invoke-SignFile {
    param([string] $File)

    $publisher = Get-RequiredEnv 'HYPERCOLOR_SIGNING_PUBLISHER'
    $existing = Get-AuthenticodeSignature -LiteralPath $File
    $signer = Get-SignerName $existing
    if ($signer -and $signer -ne $publisher) {
        # Tauri hands over any bundled exe or dll that fails its own trust
        # check. Re-signing a vendor binary would vouch for code Hypercolor
        # did not build and break PawnIO's install-time hash check.
        Write-Host "::warning title=Windows signing::Left $File with its existing signature from '$signer' (status $($existing.Status))."
        return
    }

    Invoke-SignTool $File
    $problem = Get-SignatureProblem $File $publisher
    if ($problem) {
        Write-Failure "$File did not verify after signing: $problem"
    }
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $File).Hash
    Add-Content -LiteralPath (Get-RequiredEnv 'HYPERCOLOR_SIGNING_LOG') -Value "$hash`t$File" -Encoding utf8
    Write-Host "signed and verified $File"
}

function Test-TauriPluginCopy {
    param([string] $File)

    # tauri-bundler copies NSIS's Plugins\x86-unicode next to the rendered
    # installer.nsi and signs plugins only inside that copy.
    $directory = Split-Path -Parent $File
    (Split-Path -Leaf $directory) -eq 'x86-unicode' -and
        (Split-Path -Leaf (Split-Path -Parent $directory)) -eq 'Plugins' -and
        [IO.Path]::GetExtension($File) -eq '.dll'
}

function Invoke-Sign {
    param([string] $File)

    Assert-Windows
    if (-not $File) {
        Write-Failure 'sign needs the path of the file to sign'
    }
    if (-not (Test-Path -LiteralPath $File -PathType Leaf)) {
        Write-Failure "no file to sign at $File"
    }
    $resolved = (Resolve-Path -LiteralPath $File).Path
    Invoke-SignFile $resolved

    if (Test-TauriPluginCopy $resolved) {
        $publisher = Get-RequiredEnv 'HYPERCOLOR_SIGNING_PUBLISHER'
        foreach ($plugin in $ExtraNsisPlugins) {
            $sibling = Join-Path (Split-Path -Parent $resolved) $plugin
            if (-not (Test-Path -LiteralPath $sibling)) {
                Write-Failure "the Tauri NSIS plugin copy has no $plugin, which the Hypercolor installer calls"
            }
            if (Get-SignatureProblem $sibling $publisher) {
                Invoke-SignFile $sibling
            }
        }
    }
}

function Invoke-SignPayload {
    Assert-Windows
    $repoRoot = Split-Path -Parent $PSScriptRoot
    $release = Join-Path $env:CARGO_TARGET_DIR 'release'
    $stage = Join-Path $repoRoot 'target\bundle-stage'
    $programs = @($ReleasePrograms | ForEach-Object { Join-Path $release $_ }) +
        @($StagedPrograms | ForEach-Object { Join-Path $stage $_ })
    foreach ($program in $programs) {
        Invoke-Sign $program
    }
}

function Find-SevenZip {
    $command = Get-Command 7z -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }
    $candidate = Join-Path $env:ProgramFiles '7-Zip\7z.exe'
    if (Test-Path -LiteralPath $candidate) {
        return $candidate
    }
    Write-Failure '7-Zip is required to unpack the NSIS installer'
}

function Expand-Installer {
    param([string] $File, [string] $Name)

    $tempRoot = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
    $destination = Join-Path $tempRoot "windows-signing-verify-$Name"
    if (Test-Path -LiteralPath $destination) {
        Remove-Item -LiteralPath $destination -Recurse -Force
    }
    & (Find-SevenZip) x -y "-o$destination" $File | Out-Null
    if ($LASTEXITCODE -ne 0) {
        Write-Failure "7-Zip could not extract $File"
    }
    $destination
}

function Get-RelativeFiles {
    param([string] $Root)

    # Payload paths read the way the installer lays them out on Windows.
    $prefix = $Root.TrimEnd('\', '/').Length + 1
    Get-ChildItem -LiteralPath $Root -Recurse -File -Force |
        ForEach-Object { $_.FullName.Substring($prefix).Replace('/', '\') } |
        Sort-Object
}

function Test-PeFile {
    param([string] $RelativePath)

    [IO.Path]::GetExtension($RelativePath) -in '.exe', '.dll'
}

function Invoke-Verify {
    Assert-Windows
    $publisher = if ($env:HYPERCOLOR_SIGNING_PUBLISHER) {
        $env:HYPERCOLOR_SIGNING_PUBLISHER
    } else {
        "$env:WINDOWS_SIGNING_PUBLISHER".Trim()
    }
    if (-not $publisher) {
        Write-Failure 'verify needs HYPERCOLOR_SIGNING_PUBLISHER or WINDOWS_SIGNING_PUBLISHER'
    }
    if (-not $Installer -or -not (Test-Path -LiteralPath $Installer -PathType Leaf)) {
        Write-Failure "no installer to verify at '$Installer'"
    }

    $failures = [Collections.Generic.List[string]]::new()
    $rows = [Collections.Generic.List[string]]::new()
    $problem = Get-SignatureProblem $Installer $publisher
    if ($problem) {
        $failures.Add("$(Split-Path -Leaf $Installer): $problem")
    }

    $root = Expand-Installer $Installer 'signed'
    $files = @(Get-RelativeFiles $root)
    foreach ($required in $RequiredSignedPayload) {
        if ($required -notin $files) {
            $failures.Add("$required is missing from the installer")
        }
    }
    foreach ($relative in ($files | Where-Object { Test-PeFile $_ })) {
        $full = Join-Path $root $relative
        $signature = Get-AuthenticodeSignature -LiteralPath $full
        $signer = Get-SignerName $signature
        $rows.Add("| ``$relative`` | $(if ($signer) { $signer } else { '(none)' }) | $($signature.Status) |")
        $leaf = Split-Path -Leaf $relative
        if ($leaf -in $MicrosoftRuntimeBinaries) {
            if ($signature.Status -ne 'Valid' -or $signer -ne $MicrosoftSigner) {
                $failures.Add("$relative is a VC++ runtime but carries a $($signature.Status) signature from '$signer', expected a Valid one from '$MicrosoftSigner'")
            }
            continue
        }
        if ($leaf -in $ThirdPartyBinaries) {
            if (-not $signer) {
                $failures.Add("$relative is a vendor binary but carries no signature")
            } elseif ($signer -eq $publisher) {
                $failures.Add("$relative is a vendor binary but carries the Hypercolor signature")
            }
            continue
        }
        $problem = Get-SignatureProblem $full $publisher
        if ($problem) {
            $failures.Add("${relative}: $problem")
        }
    }

    # PawnIO's setup must reach users byte for byte, or its install fails.
    $manifestPath = Join-Path $root "$PayloadUnit\tools\pawnio\manifest.json"
    $pawnIoSetup = Join-Path $root "$PayloadUnit\tools\pawnio\PawnIO_setup.exe"
    if ((Test-Path -LiteralPath $manifestPath) -and (Test-Path -LiteralPath $pawnIoSetup)) {
        $pinned = (Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json).pawnio_setup.sha256
        $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $pawnIoSetup).Hash
        if ($actual -ne $pinned) {
            $failures.Add("$PayloadUnit\tools\pawnio\PawnIO_setup.exe hash $actual differs from its pin $pinned")
        }
    } else {
        $failures.Add("the installer is missing $PayloadUnit\tools\pawnio\PawnIO_setup.exe or its manifest")
    }

    # The unsigned build of the same inputs passed the upgrade check, so the
    # signed one must ship the same files, with only Hypercolor's own
    # executables changed: vendor binaries must match byte for byte.
    if ($UnsignedInstaller) {
        $unsignedRoot = Expand-Installer $UnsignedInstaller 'unsigned'
        $unsignedFiles = @(Get-RelativeFiles $unsignedRoot)
        foreach ($extra in ($files | Where-Object { $_ -notin $unsignedFiles })) {
            $failures.Add("$extra is in the signed installer but not the unsigned one")
        }
        foreach ($lost in ($unsignedFiles | Where-Object { $_ -notin $files })) {
            $failures.Add("$lost is in the unsigned installer but not the signed one")
        }
        # 7-Zip may also emit [NSIS].nsi, a script it decompiles from the
        # installer header rather than a file the installer ships. The unit's
        # manifest holds every program's hash, so it moves with their
        # signatures.
        $comparable = $files | Where-Object {
            $_ -in $unsignedFiles -and -not $_.StartsWith('[NSIS]') -and
                $_ -ne "$PayloadUnit\manifest.json" -and
                (-not (Test-PeFile $_) -or (Split-Path -Leaf $_) -in $ThirdPartyBinaries)
        }
        foreach ($relative in $comparable) {
            $signedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $root $relative)).Hash
            $unsignedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $unsignedRoot $relative)).Hash
            if ($signedHash -ne $unsignedHash) {
                $failures.Add("$relative differs between the unsigned and signed installers")
            }
        }
    }

    $summary = @(
        '### Windows signatures',
        '',
        "Installer: ``$(Split-Path -Leaf $Installer)``, publisher ``$publisher``",
        '',
        '| Payload binary | Signer | Status |',
        '| --- | --- | --- |'
    ) + $rows
    if ($env:HYPERCOLOR_SIGNING_LOG -and (Test-Path -LiteralPath $env:HYPERCOLOR_SIGNING_LOG)) {
        $signed = @(Get-Content -LiteralPath $env:HYPERCOLOR_SIGNING_LOG | Where-Object { $_ })
        $summary += @('', "Files signed during this job: $($signed.Count)")
    }
    Add-GitHubFile 'GITHUB_STEP_SUMMARY' ($summary -join "`n")
    $summary | ForEach-Object { Write-Host $_ }

    if ($failures.Count -gt 0) {
        foreach ($failure in $failures) {
            Write-Host "::error title=Windows signing::$failure"
        }
        Write-Failure "$($failures.Count) signature check(s) failed for $Installer"
    }
    Write-Host "every binary in $(Split-Path -Leaf $Installer) carries the expected signature"
}

function Invoke-Selftest {
    Assert-Windows
    if (-not $Path -or -not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        Write-Failure "selftest needs an unsigned probe binary, got '$Path'"
    }
    $publisher = Get-RequiredEnv 'HYPERCOLOR_SIGNING_PUBLISHER'
    $tempRoot = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
    $work = Join-Path $tempRoot 'windows-signing-selftest'
    if (Test-Path -LiteralPath $work) {
        Remove-Item -LiteralPath $work -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $work | Out-Null

    $direct = Join-Path $work 'probe.exe'
    Copy-Item -LiteralPath $Path -Destination $direct
    Invoke-Sign $direct

    # makensis signs the uninstaller through `cmd.exe /C IF 1==1 <line>`,
    # where <line> is Tauri's rendering of the signCommand config, on a
    # GetTempFileName file with no .exe extension. Replay exactly that.
    $hooked = Join-Path $work 'nst5E1F.tmp'
    Copy-Item -LiteralPath $Path -Destination $hooked
    $config = Get-Content -Raw -LiteralPath (Get-RequiredEnv 'HYPERCOLOR_TAURI_SIGN_CONFIG') | ConvertFrom-Json
    $sign = $config.bundle.windows.signCommand
    $line = (Format-SignCommandLine (@($sign.cmd) + @($sign.args))).Replace('%1', $hooked)
    $start = [Diagnostics.ProcessStartInfo]::new($env:ComSpec, "/C IF 1==1 $line")
    $start.UseShellExecute = $false
    $process = [Diagnostics.Process]::Start($start)
    $process.WaitForExit()
    if ($process.ExitCode -ne 0) {
        Write-Failure "the uninstaller hook command exited $($process.ExitCode): $line"
    }
    $problem = Get-SignatureProblem $hooked $publisher
    if ($problem) {
        Write-Failure "the uninstaller hook left $hooked unverified: $problem"
    }

    $signature = Get-AuthenticodeSignature -LiteralPath $direct
    $summary = @(
        '### Windows signing selftest',
        '',
        "Signer: ``$(Get-SignerName $signature)``",
        "Issuer: ``$($signature.SignerCertificate.Issuer)``",
        "Timestamped by: ``$($signature.TimeStamperCertificate.Subject)``",
        '',
        'Signed a probe directly and through the uninstaller hook command line.'
    )
    Add-GitHubFile 'GITHUB_STEP_SUMMARY' ($summary -join "`n")
    $summary | ForEach-Object { Write-Host $_ }
}

switch ($Command) {
    'resolve' { Invoke-Resolve }
    'setup' { Invoke-Setup }
    'sign' { Invoke-Sign $Path }
    'sign-payload' { Invoke-SignPayload }
    'verify' { Invoke-Verify }
    'selftest' { Invoke-Selftest }
}
