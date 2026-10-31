<#
.SYNOPSIS
  Smoke-test the Windows release artifacts: the portable .zip and the NSIS installer.

.DESCRIPTION
  1. Extracts the .zip, checks the embedded version resource and runs `ferrocord.exe --version`.
  2. Silently installs the setup.exe into a temporary directory (per-user, no UAC), checks the
     files, Start-menu shortcut and Add/Remove Programs entry, runs the installed exe, then
     silently uninstalls and checks that everything was removed.

  ferrocord.exe is a GUI-subsystem program in release builds, so its stdout is captured through
  a redirected pipe (Start-Process -RedirectStandardOutput) rather than the console.

.EXAMPLE
  ./packaging/windows/smoke-test.ps1 -Zip dist/ferrocord-0.1.0-windows-x86_64.zip `
      -Installer dist/ferrocord-0.1.0-windows-x86_64-setup.exe -Version 0.1.0
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Zip,
    [Parameter(Mandatory)] [string] $Installer,
    [Parameter(Mandatory)] [string] $Version
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-Version([string] $Exe) {
    $out = New-TemporaryFile
    $err = New-TemporaryFile
    $p = Start-Process -FilePath $Exe -ArgumentList '--version' -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput $out.FullName -RedirectStandardError $err.FullName
    $text = (Get-Content -Raw $out.FullName).Trim()
    $errText = (Get-Content -Raw $err.FullName)
    Remove-Item $out, $err
    if ($p.ExitCode -ne 0) { throw "$Exe --version exited with $($p.ExitCode): $errText" }
    if ($text -ne "ferrocord $Version") { throw "$Exe --version printed '$text', expected 'ferrocord $Version'" }
    Write-Host "ok: $Exe --version -> $text"
}

function Assert-VersionInfo([string] $Exe) {
    $vi = (Get-Item $Exe).VersionInfo
    if ($vi.ProductVersion -ne $Version) { throw "ProductVersion is '$($vi.ProductVersion)', expected '$Version'" }
    if ($vi.ProductName -ne 'Ferrocord') { throw "ProductName is '$($vi.ProductName)'" }
    $core = ($Version -split '[-+]')[0]
    if ("$($vi.FileMajorPart).$($vi.FileMinorPart).$($vi.FileBuildPart)" -ne $core) {
        throw "numeric file version $($vi.FileVersionRaw) does not match $core"
    }
    Add-Type -AssemblyName System.Drawing
    if (-not [System.Drawing.Icon]::ExtractAssociatedIcon((Resolve-Path $Exe))) { throw "no icon in $Exe" }
    Write-Host "ok: version resource $($vi.ProductName) $($vi.ProductVersion) ($($vi.FileVersionRaw))"
}

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("ferrocord-smoke-" + [guid]::NewGuid())
New-Item -ItemType Directory $work | Out-Null
try {
    # --- portable zip -------------------------------------------------------
    Expand-Archive -Path $Zip -DestinationPath (Join-Path $work 'zip')
    $exe = Get-ChildItem -Path (Join-Path $work 'zip') -Recurse -Filter ferrocord.exe | Select-Object -First 1
    if (-not $exe) { throw "ferrocord.exe not found in $Zip" }
    Assert-VersionInfo $exe.FullName
    Invoke-Version $exe.FullName

    # --- installer ----------------------------------------------------------
    $dir = Join-Path $work 'installed'
    $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\dev.ferrocord.Ferrocord'
    $lnk = Join-Path ([Environment]::GetFolderPath('Programs')) 'Ferrocord.lnk'

    $p = Start-Process -FilePath (Resolve-Path $Installer) -ArgumentList '/S', "/D=$dir" -Wait -PassThru
    if ($p.ExitCode -ne 0) { throw "installer exited with $($p.ExitCode)" }
    foreach ($f in 'ferrocord.exe', 'uninstall.exe', 'LICENSE.txt', 'README.txt') {
        if (-not (Test-Path (Join-Path $dir $f))) { throw "installer did not create $f" }
    }
    if (-not (Test-Path $lnk)) { throw "Start menu shortcut missing: $lnk" }
    $reg = Get-ItemProperty $key
    if ($reg.DisplayVersion -ne $Version) { throw "registry DisplayVersion is '$($reg.DisplayVersion)'" }
    if ($reg.InstallLocation -ne $dir) { throw "registry InstallLocation is '$($reg.InstallLocation)'" }
    Write-Host "ok: installed to $dir ($([int]$reg.EstimatedSize) KB), shortcut and uninstall entry present"
    Invoke-Version (Join-Path $dir 'ferrocord.exe')

    # The uninstaller normally re-launches itself from %TEMP%; _?= keeps it in place so -Wait works.
    $p = Start-Process -FilePath (Join-Path $dir 'uninstall.exe') -ArgumentList '/S', "_?=$dir" -Wait -PassThru
    if ($p.ExitCode -ne 0) { throw "uninstaller exited with $($p.ExitCode)" }
    Remove-Item (Join-Path $dir 'uninstall.exe') -ErrorAction SilentlyContinue  # left behind by _?=
    if (Test-Path (Join-Path $dir 'ferrocord.exe')) { throw "uninstaller left ferrocord.exe behind" }
    if (Test-Path $lnk) { throw "uninstaller left the Start menu shortcut behind" }
    if (Test-Path $key) { throw "uninstaller left the registry key behind" }
    Write-Host "ok: uninstalled cleanly"
}
finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
