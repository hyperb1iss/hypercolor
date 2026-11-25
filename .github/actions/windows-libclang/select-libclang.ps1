param(
    [Parameter(Mandatory)]
    [int] $MinimumMajor
)

$ErrorActionPreference = 'Stop'

function Fail([string] $Message) {
    Write-Output "::error title=Windows libclang::$Message"
    exit 1
}

if (-not $IsWindows) { Fail "this action runs on Windows runners only" }
if ($env:RUNNER_ARCH -ne 'X64') { Fail "no libclang selection for runner architecture $env:RUNNER_ARCH" }

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path -LiteralPath $vswhere)) { Fail "vswhere.exe not found at $vswhere" }

# The newest install with the C++ toolchain is the one a developer shell
# enters, so its bundled LLVM is versioned with the STL headers bindgen
# parses. The standalone LLVM under Program Files moves on its own schedule,
# and clang-sys prefers it whenever LIBCLANG_PATH is unset.
$vs = & $vswhere -latest -products * `
    -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
    -property installationPath
if (-not $vs) { Fail "no Visual Studio install with the x64 C++ toolchain found" }

$toolsetFile = Join-Path $vs 'VC\Auxiliary\Build\Microsoft.VCToolsVersion.default.txt'
$toolset = if (Test-Path -LiteralPath $toolsetFile) { (Get-Content -LiteralPath $toolsetFile -Raw).Trim() } else { 'unknown' }

$bin = Join-Path $vs 'VC\Tools\Llvm\x64\bin'
$libclang = Join-Path $bin 'libclang.dll'
$clang = Join-Path $bin 'clang.exe'
foreach ($file in $libclang, $clang) {
    if (-not (Test-Path -LiteralPath $file)) {
        Fail "$file not found; Visual Studio at $vs needs the C++ Clang component (Microsoft.VisualStudio.Component.VC.Llvm.Clang)"
    }
}

# Ask the library itself: its reported version is the __clang_major__ the STL
# checks, and loading it proves an x64 process can.
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class HypercolorLibClang
{
    [StructLayout(LayoutKind.Sequential)]
    public struct CXString
    {
        public IntPtr Data;
        public uint Flags;
    }

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate CXString GetClangVersion();

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate IntPtr GetCString(CXString value);

    public static string Version(string path)
    {
        IntPtr library = NativeLibrary.Load(path);
        var getVersion = Marshal.GetDelegateForFunctionPointer<GetClangVersion>(
            NativeLibrary.GetExport(library, "clang_getClangVersion"));
        var getCString = Marshal.GetDelegateForFunctionPointer<GetCString>(
            NativeLibrary.GetExport(library, "clang_getCString"));
        return Marshal.PtrToStringUTF8(getCString(getVersion()));
    }
}
'@

$reported = [HypercolorLibClang]::Version($libclang)
if ($reported -notmatch 'clang version (\d+)\.') {
    Fail "$libclang reported no parseable version: '$reported'"
}
$major = [int] $Matches[1]
if ($major -lt $MinimumMajor) {
    Fail "$libclang is Clang $major ('$reported'), but MSVC $toolset needs Clang $MinimumMajor or newer for bindgen to parse its STL headers"
}

# CLANG_PATH keeps bindgen's include-path probe on the same Clang, so it does
# not mix the standalone LLVM's builtin headers into a newer libclang's parse.
"LIBCLANG_PATH=$bin" | Out-File -FilePath $env:GITHUB_ENV -Append -Encoding utf8
"CLANG_PATH=$clang" | Out-File -FilePath $env:GITHUB_ENV -Append -Encoding utf8
"version=$reported" | Out-File -FilePath $env:GITHUB_OUTPUT -Append -Encoding utf8

Write-Output "MSVC toolset: $toolset"
Write-Output "libclang: $reported"
Write-Output "LIBCLANG_PATH=$bin"
Write-Output "CLANG_PATH=$clang"
