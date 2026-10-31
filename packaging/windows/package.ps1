<#
.SYNOPSIS
  Package a built ferrocord.exe as a portable .zip and an .msi installer.

.DESCRIPTION
  Outputs, in -OutDir:
    ferrocord-<version>-windows-<arch>.zip   portable build (drop a file named `portable`
                                             next to the exe to keep all data beside it)
    ferrocord-<version>-windows-<arch>.msi   WiX installer with the branded WixUI dialogs

  Needs the WiX Toolset 5 .NET tool and its UI and Util extensions:
    dotnet tool install --global wix --version 5.0.2
    wix extension add --global WixToolset.UI.wixext/5.0.2 WixToolset.Util.wixext/5.0.2

  Code signing is optional. When WINDOWS_SIGN_PFX (path to a .pfx) and
  WINDOWS_SIGN_PASSWORD are set, ferrocord.exe and the .msi are signed with
  signtool and timestamped.

.EXAMPLE
  ./packaging/windows/package.ps1 -Exe target/x86_64-pc-windows-msvc/release/ferrocord.exe `
      -Version 0.1.0 -Arch x86_64 -OutDir dist
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Exe,
    [Parameter(Mandatory)] [string] $Version,
    [Parameter(Mandatory)] [ValidateSet('x86_64', 'arm64')] [string] $Arch,
    [Parameter(Mandatory)] [string] $OutDir,
    # `wix` on PATH, or a path to wix.exe / wix.dll.
    [string] $Wix = 'wix',
    # Extension names (from the global WiX cache) or paths to the extension DLLs.
    [string] $UiExtension = 'WixToolset.UI.wixext',
    [string] $UtilExtension = 'WixToolset.Util.wixext'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = Resolve-Path (Join-Path $PSScriptRoot '../..')
$core = ($Version -split '[-+]')[0]
if ($core -notmatch '^\d+\.\d+\.\d+$') { throw "version '$Version' is not SemVer" }
$wixArch = @{ x86_64 = 'x64'; arm64 = 'arm64' }[$Arch]
$name = "ferrocord-$Version-windows-$Arch"

New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$stage = Join-Path ([IO.Path]::GetTempPath()) ("ferrocord-pkg-" + [guid]::NewGuid())
$files = Join-Path $stage $name
$art = Join-Path $stage 'art'
New-Item -ItemType Directory $files, $art | Out-Null

function Invoke-Sign([string] $Path) {
    if (-not $env:WINDOWS_SIGN_PFX) { return }
    $signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" |
        Sort-Object FullName -Descending | Select-Object -First 1
    if (-not $signtool) { throw 'signtool.exe not found (install the Windows SDK)' }
    & $signtool.FullName sign /f $env:WINDOWS_SIGN_PFX /p $env:WINDOWS_SIGN_PASSWORD `
        /fd sha256 /tr 'http://timestamp.digicert.com' /td sha256 /d 'Ferrocord' $Path
    if ($LASTEXITCODE -ne 0) { throw "signing $Path failed" }
}

# Plain text with CRLF line endings so Notepad shows it properly.
function Write-CrLf([string] $From, [string] $To) {
    $text = (Get-Content -Raw $From) -replace "`r?`n", "`r`n"
    [IO.File]::WriteAllText($To, $text, [Text.UTF8Encoding]::new($false))
}

# The WixUI license page only takes RTF; build it from LICENSE so the two never drift.
function Write-LicenseRtf([string] $From, [string] $To) {
    $paras = ((Get-Content -Raw $From) -replace "`r", '') -split "`n\s*`n" | ForEach-Object {
        $p = ($_.Trim() -replace '\s*\n\s*', ' ') -replace '\\', '\\' -replace '\{', '\{' -replace '\}', '\}'
        $p = [regex]::Replace($p, '[^\x00-\x7f]', { param($m) '\u' + [int][char]$m.Value + '?' })
        if ($p -match '^\d+\.') { "\pard\li280\fi-280\sa120 $p\par" } else { "\pard\sa120 $p\par" }
    }
    $rtf = "{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0\fswiss Segoe UI;}}\viewkind4\uc1\f0\fs17`r`n" +
        ($paras -join "`r`n") + "`r`n}"
    [IO.File]::WriteAllText($To, $rtf, [Text.Encoding]::ASCII)
}

function Invoke-Wix([string[]] $Arguments) {
    if ($Wix -like '*.dll') {
        # A framework-dependent wix.dll (e.g. unpacked from the NuGet package) runs on any newer .NET.
        $env:DOTNET_ROLL_FORWARD = 'Major'
        & dotnet $Wix @Arguments
    } else {
        & $Wix @Arguments
    }
    if ($LASTEXITCODE -ne 0) { throw "wix $($Arguments[0]) failed with exit code $LASTEXITCODE" }
}

try {
    Copy-Item $Exe (Join-Path $files 'ferrocord.exe')
    Invoke-Sign (Join-Path $files 'ferrocord.exe')
    Write-CrLf (Join-Path $root 'LICENSE') (Join-Path $files 'LICENSE.txt')
    Write-CrLf (Join-Path $root 'README.md') (Join-Path $files 'README.txt')

    # --- portable zip ----------------------------------------------------------
    $zip = Join-Path $OutDir "$name.zip"
    Remove-Item $zip -ErrorAction SilentlyContinue
    Compress-Archive -Path $files -DestinationPath $zip -CompressionLevel Optimal
    Write-Host "created $zip"

    # --- msi -------------------------------------------------------------------
    Copy-Item (Join-Path $root 'packaging/windows/wix/*.bmp') $art
    Write-LicenseRtf (Join-Path $root 'LICENSE') (Join-Path $art 'license.rtf')
    $msi = Join-Path $OutDir "$name.msi"
    Invoke-Wix @(
        'build', (Join-Path $root 'packaging/windows/ferrocord.wxs'),
        '-arch', $wixArch,
        '-ext', $UiExtension,
        '-ext', $UtilExtension,
        '-d', "Version=$core",
        '-d', "DisplayVersion=$Version",
        '-d', "SourceDir=$files",
        '-d', "ArtDir=$art",
        '-d', "IconFile=$(Join-Path $root 'packaging/icons/ferrocord.ico')",
        '-pdbtype', 'none',
        '-o', $msi
    )
    # Internal consistency evaluators. Two are suppressed on purpose:
    #   ICE57  flags HKMU key paths, which are exactly how a per-user-or-machine package must
    #          record per-user data (they resolve to HKCU or HKLM with the install scope).
    #   ICE69  flags shortcuts whose target is in another feature; the shortcut features are
    #          children of the always-installed Main feature, so the target is always present.
    Invoke-Wix @('msi', 'validate', '-sice', 'ICE57', '-sice', 'ICE69', $msi)
    Invoke-Sign $msi
    Write-Host "created $msi"
}
finally {
    Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
}
