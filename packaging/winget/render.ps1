<#
.SYNOPSIS
  Render winget manifests (Ferrocord.Ferrocord) for a release's .msi installers.

.DESCRIPTION
  Writes the three-file multi-manifest (version, defaultLocale, installer) to
  -OutDir/<version>/. Each MSI is listed twice, once per install scope, because one
  package serves both: per-user by default, per-machine with ALLUSERS=1.

  The installer URLs point at the GitHub release asset names produced by
  packaging/windows/package.ps1, so render after packaging and submit after the
  release is published (wingetcreate submit, see docs/RELEASING.md).

.EXAMPLE
  ./packaging/winget/render.ps1 -Version 0.2.0 -Repository ferrocord/ferrocord `
      -X64Msi dist/ferrocord-0.2.0-windows-x86_64.msi -Arm64Msi dist/ferrocord-0.2.0-windows-arm64.msi `
      -OutDir winget
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Version,
    [Parameter(Mandatory)] [string] $Repository,
    [Parameter(Mandatory)] [string] $X64Msi,
    [Parameter(Mandatory)] [string] $Arm64Msi,
    [Parameter(Mandatory)] [string] $OutDir
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$id = 'Ferrocord.Ferrocord'
$schema = '1.10.0'
$upgradeCode = '{6F2B9C4E-3A1D-4E7B-9F0C-2D8E5A7B1C93}'

function Get-MsiProperty([string] $Path, [string] $Name) {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    $db = $installer.GetType().InvokeMember('OpenDatabase', 'InvokeMethod', $null, $installer, @($Path, 0))
    $view = $db.GetType().InvokeMember('OpenView', 'InvokeMethod', $null, $db, @("SELECT Value FROM Property WHERE Property = '$Name'"))
    [void]$view.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $view, $null)
    $record = $view.GetType().InvokeMember('Fetch', 'InvokeMethod', $null, $view, $null)
    $value = $record.GetType().InvokeMember('StringData', 'GetProperty', $null, $record, 1)
    [void]$view.GetType().InvokeMember('Close', 'InvokeMethod', $null, $view, $null)
    # Release every handle, or the .msi stays locked until this PowerShell session exits.
    foreach ($o in @($record, $view, $db, $installer)) { if ($o) { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($o) } }
    $value
}

function Write-Manifest([string] $Path, [string] $Type, [string] $Body) {
    $header = "# yaml-language-server: `$schema=https://aka.ms/winget-manifest.$Type.$schema.schema.json`n`n"
    [IO.File]::WriteAllText($Path, $header + $Body.Trim() + "`n", [Text.UTF8Encoding]::new($false))
}

$dir = Join-Path $OutDir $Version
New-Item -ItemType Directory -Force $dir | Out-Null
$tag = "v$Version"

$installers = foreach ($entry in @(@{ Arch = 'x64'; Msi = $X64Msi }, @{ Arch = 'arm64'; Msi = $Arm64Msi })) {
    $msi = (Resolve-Path $entry.Msi).Path
    $file = Split-Path $msi -Leaf
    $sha = (Get-FileHash -Algorithm SHA256 $msi).Hash
    $productCode = Get-MsiProperty $msi 'ProductCode'
    $msiVersion = Get-MsiProperty $msi 'ProductVersion'
    $url = "https://github.com/$Repository/releases/download/$tag/$file"
    foreach ($scope in 'user', 'machine') {
        $switches = if ($scope -eq 'machine') { "`n  InstallerSwitches:`n    Custom: ALLUSERS=1" } else { '' }
        @"
- Architecture: $($entry.Arch)
  Scope: $scope
  InstallerUrl: $url
  InstallerSha256: $sha
  ProductCode: '$productCode'$switches
  AppsAndFeaturesEntries:
  - DisplayName: Ferrocord
    DisplayVersion: $msiVersion
    ProductCode: '$productCode'
    UpgradeCode: '$upgradeCode'
"@
    }
}

Write-Manifest (Join-Path $dir "$id.yaml") 'version' @"
PackageIdentifier: $id
PackageVersion: $Version
DefaultLocale: en-US
ManifestType: version
ManifestVersion: $schema
"@

Write-Manifest (Join-Path $dir "$id.locale.en-US.yaml") 'defaultLocale' @"
PackageIdentifier: $id
PackageVersion: $Version
PackageLocale: en-US
Publisher: Ferrocord Contributors
PublisherUrl: https://github.com/$Repository
PublisherSupportUrl: https://github.com/$Repository/issues
PackageName: Ferrocord
PackageUrl: https://github.com/$Repository
License: BSD-3-Clause
LicenseUrl: https://github.com/$Repository/blob/$tag/LICENSE
ShortDescription: A fast, open-source Discord client written in Rust.
Description: |-
  Ferrocord is a lightweight desktop client for Discord with text chat, voice and video
  calls, Luau plugins and a local automation API.
Tags:
- chat
- discord
- rust
- voice
ReleaseNotesUrl: https://github.com/$Repository/releases/tag/$tag
ManifestType: defaultLocale
ManifestVersion: $schema
"@

Write-Manifest (Join-Path $dir "$id.installer.yaml") 'installer' @"
PackageIdentifier: $id
PackageVersion: $Version
MinimumOSVersion: 10.0.17763.0
InstallerType: wix
UpgradeBehavior: install
ReleaseDate: $((Get-Date).ToUniversalTime().ToString('yyyy-MM-dd'))
Installers:
$($installers -join "`n")
ManifestType: installer
ManifestVersion: $schema
"@

Get-ChildItem $dir | ForEach-Object { Write-Host "rendered $($_.FullName)" }
