<#
.SYNOPSIS
  Smoke-test the Windows release artifacts: the portable .zip and the .msi installer.

.DESCRIPTION
  1. Extracts the .zip, checks the embedded version resource and runs `ferrocord.exe --version`
     and `--paths` (in portable mode, so the paths must point beside the exe).
  2. Reads the .msi's properties, silently installs it per-user (no UAC), checks the files,
     shortcuts, App Paths and Apps & Features entries, runs the installed exe, then silently
     uninstalls and checks that everything was removed.

  ferrocord.exe is a GUI-subsystem program in release builds, so its stdout is captured through
  a redirected pipe (Start-Process -RedirectStandardOutput) rather than the console.

  The install is a real per-user install into %LOCALAPPDATA%\Programs\Ferrocord, so the script
  refuses to run if Ferrocord is already installed for the current user.

  With -UpgradeFromMsi, that older package is installed first and the new one must replace it
  (MajorUpgrade) rather than install side by side.

.EXAMPLE
  ./packaging/windows/smoke-test.ps1 -Zip dist/ferrocord-0.1.0-windows-x86_64.zip `
      -Msi dist/ferrocord-0.1.0-windows-x86_64.msi -Version 0.1.0
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Zip,
    [Parameter(Mandatory)] [string] $Msi,
    [Parameter(Mandatory)] [string] $Version,
    # An older release's .msi: installed first, so the new one must upgrade it in place.
    [string] $UpgradeFromMsi
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-Exe([string] $Exe, [string[]] $Arguments) {
    $out = New-TemporaryFile
    $err = New-TemporaryFile
    $p = Start-Process -FilePath $Exe -ArgumentList $Arguments -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput $out.FullName -RedirectStandardError $err.FullName
    $text = (Get-Content -Raw $out.FullName)
    $errText = (Get-Content -Raw $err.FullName)
    Remove-Item $out, $err
    if ($p.ExitCode -ne 0) { throw "$Exe $Arguments exited with $($p.ExitCode): $errText" }
    if ($text) { $text.Trim() } else { '' }
}

function Assert-Version([string] $Exe) {
    $text = Invoke-Exe $Exe @('--version')
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

function Get-MsiProperty([string] $Path, [string] $Name) {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    $db = $installer.GetType().InvokeMember('OpenDatabase', 'InvokeMethod', $null, $installer, @($Path, 0))
    $view = $db.GetType().InvokeMember('OpenView', 'InvokeMethod', $null, $db, @("SELECT Value FROM Property WHERE Property = '$Name'"))
    [void]$view.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $view, $null)
    $record = $view.GetType().InvokeMember('Fetch', 'InvokeMethod', $null, $view, $null)
    $value = if ($record) { $record.GetType().InvokeMember('StringData', 'GetProperty', $null, $record, 1) } else { $null }
    [void]$view.GetType().InvokeMember('Close', 'InvokeMethod', $null, $view, $null)
    # Release every handle, or the .msi stays locked until this PowerShell session exits.
    foreach ($o in @($record, $view, $db, $installer)) { if ($o) { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($o) } }
    $value
}

# Installed-product data from Windows Installer (per-user MSI installs are registered there,
# not under the HKCU Uninstall key).
function Get-ProductInfo([string] $ProductCode, [string] $Attribute) {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    try { $installer.GetType().InvokeMember('ProductInfo', 'GetProperty', $null, $installer, @($ProductCode, $Attribute)) }
    finally { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer) | Out-Null }
}

function Get-ProductState([string] $ProductCode) {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    try { $installer.GetType().InvokeMember('ProductState', 'GetProperty', $null, $installer, @($ProductCode)) }
    finally { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer) | Out-Null }
}

function Get-RelatedProductCode([string] $UpgradeCode) {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    try { $installer.GetType().InvokeMember('RelatedProducts', 'GetProperty', $null, $installer, @($UpgradeCode)) | ForEach-Object { "$_" } }
    finally { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer) | Out-Null }
}

function Invoke-Msiexec([string[]] $Arguments, [string] $Log) {
    $p = Start-Process -FilePath msiexec.exe -ArgumentList ($Arguments + @('/qn', '/norestart', '/l*v', "`"$Log`"")) -Wait -PassThru
    if ($p.ExitCode -notin 0, 3010) {
        Get-Content $Log -Tail 60 | Write-Host
        throw "msiexec $($Arguments[0]) exited with $($p.ExitCode) (log: $Log)"
    }
}

$work = Join-Path ([IO.Path]::GetTempPath()) ("ferrocord-smoke-" + [guid]::NewGuid())
New-Item -ItemType Directory $work | Out-Null
try {
    # --- portable zip ---------------------------------------------------------
    Expand-Archive -Path $Zip -DestinationPath (Join-Path $work 'zip')
    $exe = Get-ChildItem -Path (Join-Path $work 'zip') -Recurse -Filter ferrocord.exe | Select-Object -First 1
    if (-not $exe) { throw "ferrocord.exe not found in $Zip" }
    foreach ($f in 'LICENSE.txt', 'README.txt') {
        if (-not (Test-Path (Join-Path $exe.DirectoryName $f))) { throw "$f missing from $Zip" }
    }
    Assert-VersionInfo $exe.FullName
    Assert-Version $exe.FullName
    New-Item -ItemType File (Join-Path $exe.DirectoryName 'portable') | Out-Null
    $paths = Invoke-Exe $exe.FullName @('--paths')
    $expected = Join-Path $exe.DirectoryName 'data'
    foreach ($line in $paths -split "`r?`n") {
        if (-not $line.Substring(7).StartsWith($expected)) { throw "portable mode path outside $expected`: $line" }
    }
    Write-Host "ok: portable mode keeps data in $expected"

    # --- msi ------------------------------------------------------------------
    $Msi = (Resolve-Path $Msi).Path
    $productVersion = Get-MsiProperty $Msi 'ProductVersion'
    $productCode = Get-MsiProperty $Msi 'ProductCode'
    $upgradeCode = Get-MsiProperty $Msi 'UpgradeCode'
    if ($productVersion -ne ($Version -split '[-+]')[0]) { throw "MSI ProductVersion is $productVersion" }
    if ($upgradeCode -ne '{6F2B9C4E-3A1D-4E7B-9F0C-2D8E5A7B1C93}') { throw "MSI UpgradeCode changed: $upgradeCode" }
    if ((Get-MsiProperty $Msi 'ALLUSERS') -ne '2' -or (Get-MsiProperty $Msi 'MSIINSTALLPERUSER') -ne '1') {
        throw 'MSI is not a per-user-or-machine package'
    }
    Write-Host "ok: msi $productVersion, product code $productCode"

    $dir = Join-Path $env:LOCALAPPDATA 'Programs\Ferrocord'
    $installedExe = Join-Path $dir 'ferrocord.exe'
    $startLnk = Join-Path ([Environment]::GetFolderPath('Programs')) 'Ferrocord.lnk'
    $desktopLnk = Join-Path ([Environment]::GetFolderPath('Desktop')) 'Ferrocord.lnk'
    $appPaths = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths\ferrocord.exe'
    if ((Test-Path $installedExe) -or (Test-Path 'HKCU:\Software\Ferrocord')) {
        throw "Ferrocord is already installed for this user ($dir); uninstall it before running the smoke test"
    }

    if ($UpgradeFromMsi) {
        $UpgradeFromMsi = (Resolve-Path $UpgradeFromMsi).Path
        Invoke-Msiexec @('/i', "`"$UpgradeFromMsi`"") (Join-Path $work 'install-previous.log')
        Write-Host "ok: installed previous release $(Get-MsiProperty $UpgradeFromMsi 'ProductVersion')"
    }
    Invoke-Msiexec @('/i', "`"$Msi`"") (Join-Path $work 'install.log')
    try {
        $related = @(Get-RelatedProductCode $upgradeCode)
        if ($related.Count -ne 1 -or $related[0] -ne $productCode) { throw "expected only $productCode to be installed, found: $related" }
        foreach ($f in $installedExe, (Join-Path $dir 'LICENSE.txt'), $startLnk, $desktopLnk) {
            if (-not (Test-Path $f)) { throw "installer did not create $f" }
        }
        $shell = New-Object -ComObject WScript.Shell
        $target = $shell.CreateShortcut($startLnk).TargetPath
        if ($target -ne $installedExe) { throw "Start menu shortcut points at '$target'" }
        if ((Get-ProductState $productCode) -ne 5) { throw "Windows Installer does not report $productCode as installed" }
        if ((Get-ProductInfo $productCode 'ProductName') -ne 'Ferrocord') { throw 'installed product name is wrong' }
        if ((Get-ProductInfo $productCode 'VersionString') -ne $productVersion) { throw 'installed product version is wrong' }
        if ((Get-ProductInfo $productCode 'AssignmentType') -ne '0') { throw 'the default install is not per-user' }
        if ((Get-ItemProperty $appPaths).'(default)' -ne $installedExe) { throw 'App Paths entry is wrong' }
        Write-Host "ok: installed per-user to $dir with shortcuts, App Paths and Windows Installer registration"
        Assert-Version $installedExe
    }
    finally {
        Invoke-Msiexec @('/x', $productCode) (Join-Path $work 'uninstall.log')
    }
    if (@(Get-RelatedProductCode $upgradeCode).Count -ne 0) { throw 'Windows Installer still lists Ferrocord after uninstall' }
    foreach ($f in $installedExe, $startLnk, $desktopLnk, $appPaths, 'HKCU:\Software\Ferrocord') {
        if (Test-Path $f) { throw "uninstall left $f behind" }
    }
    if (Test-Path $dir) { throw "uninstall left $dir behind" }
    Write-Host 'ok: uninstalled cleanly'
}
finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
