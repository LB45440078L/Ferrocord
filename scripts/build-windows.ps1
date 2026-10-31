<#
.SYNOPSIS
  Build every Ferrocord package this Windows PC can make: .msi and .zip for x64 and
  ARM64, and (through WSL2) the Linux .deb, .rpm, .AppImage and .tar.gz.

.DESCRIPTION
  Just run it (or double-click build-windows.cmd). It checks for every tool it needs,
  offers to install what is missing (with winget; Windows asks for permission when an
  installer needs administrator rights), builds, and puts all packages in .\dist.

  macOS disk images cannot be built on Windows; run scripts/build-macos.sh on a Mac.

.PARAMETER Yes
  Install missing tools without asking.
.PARAMETER Arch
  Which Windows packages to build: both (default), x64 or arm64.
.PARAMETER Quick
  Use the faster `release` profile instead of `dist` (fat LTO, what releases use).
.PARAMETER SkipLinux
  Do not build the Linux packages.
.PARAMETER LinuxOnly
  Only build the Linux packages (through WSL2).
.PARAMETER Distro
  WSL distribution to use (default: the first Ubuntu or Debian one).
.PARAMETER OutDir
  Where to put the packages (default: <repo>\dist).

.EXAMPLE
  .\scripts\build-windows.ps1
.EXAMPLE
  .\scripts\build-windows.ps1 -Arch x64 -SkipLinux -Quick
#>
[CmdletBinding()]
param(
    [switch] $Yes,
    [ValidateSet('both', 'x64', 'arm64')] [string] $Arch = 'both',
    [switch] $Quick,
    [switch] $SkipLinux,
    [switch] $LinuxOnly,
    [string] $Distro,
    [string] $OutDir
)

# Native tools write progress to stderr; with 'Stop' Windows PowerShell 5.1 would treat that as fatal.
$ErrorActionPreference = 'Continue'
Set-StrictMode -Version Latest
# Older consoles cannot switch; emojis then show as boxes, which is harmless.
try { [Console]::OutputEncoding = [Text.Encoding]::UTF8 } catch { $null = $_ }

# --- look and feel -------------------------------------------------------------
function E([int] $CodePoint) { [char]::ConvertFromUtf32($CodePoint) }
$Icons = @{
    Ok = (E 0x2705); Fail = (E 0x274C); Warn = (E 0x26A0) + ' '; Step = (E 0x1F449); Ask = (E 0x2753)
    Search = (E 0x1F50E); Tools = (E 0x1F9F0); Hammer = (E 0x1F528); Box = (E 0x1F4E6); Penguin = (E 0x1F427)
    Flag = (E 0x1F3C1); Party = (E 0x1F389); Sad = (E 0x1F615); Folder = (E 0x1F4C2); Crab = (E 0x1F980)
    Window = (E 0x1FA9F); Clock = (E 0x23F1) + ' '
}
function Write-Title([string] $Text) { Write-Host ''; Write-Host "== $Text ==" -ForegroundColor Cyan }
function Write-Step([string] $Text) { Write-Host "$($Icons.Step) $Text" -ForegroundColor Blue }
function Write-Ok([string] $Text) { Write-Host "$($Icons.Ok) $Text" -ForegroundColor Green }
function Write-Warn([string] $Text) { Write-Host "$($Icons.Warn) $Text" -ForegroundColor Yellow }
function Write-Fail([string] $Text) { Write-Host "$($Icons.Fail) $Text" -ForegroundColor Red }
function Write-Hint([string] $Text) { Write-Host "   $Text" -ForegroundColor DarkGray }

function Confirm-Action([string] $Question) {
    if ($Yes) { return $true }
    Write-Host "$($Icons.Ask) $Question [Y/n] " -ForegroundColor White -NoNewline
    try { $reply = Read-Host -ErrorAction Stop }
    catch {
        # No keyboard (non-interactive session): never install anything without an answer.
        Write-Host 'n (no input available; pass -Yes to install automatically)'
        return $false
    }
    return (-not $reply) -or ($reply -match '^[yY]')
}

function Test-Tool([string] $Name) { [bool](Get-Command $Name -ErrorAction SilentlyContinue) }

function Import-SystemPath {
    $machine = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    $user = [Environment]::GetEnvironmentVariable('Path', 'User')
    $extra = @("$HOME\.cargo\bin", "$HOME\.dotnet\tools", "$env:ProgramFiles\dotnet", "$env:ProgramFiles\CMake\bin", "$env:ProgramFiles\LLVM\bin")
    $env:Path = (@($machine, $user) + $extra | Where-Object { $_ }) -join ';'
}

function Exit-WithError([string] $Text, [string] $Help) {
    Write-Fail $Text
    if ($Help) { Write-Hint $Help }
    Write-Host ''
    exit 1
}

function Install-WithWinget([string] $Id, [string] $Name, [string[]] $Extra = @()) {
    if (-not (Test-Tool winget)) {
        Exit-WithError "winget is not available, so $Name cannot be installed automatically." `
            'Install "App Installer" from the Microsoft Store, or install the tool by hand and run this script again.'
    }
    Write-Step "Installing $Name (Windows may ask for permission)..."
    & winget install --id $Id --exact --source winget --accept-source-agreements --accept-package-agreements @Extra
    # 0 = installed; -1978335189 = already installed / no newer version.
    if ($LASTEXITCODE -notin 0, -1978335189) { Exit-WithError "Installing $Name failed (winget exit code $LASTEXITCODE)." }
    Import-SystemPath
    Write-Ok "$Name installed"
}

# --- setup -----------------------------------------------------------------------
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if (-not $OutDir) { $OutDir = Join-Path $repo 'dist' }
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$profileName = if ($Quick) { 'release' } else { 'dist' }
$started = Get-Date
Import-SystemPath

Write-Host ''
Write-Host "$($Icons.Crab) Ferrocord package builder for Windows" -ForegroundColor White
Write-Hint "Packages will be written to: $OutDir"

$archs = @()
if (-not $LinuxOnly) {
    if ($Arch -in 'both', 'x64') { $archs += [pscustomobject]@{ Name = 'x86_64'; Label = 'x64'; Target = 'x86_64-pc-windows-msvc' } }
    if ($Arch -in 'both', 'arm64') { $archs += [pscustomobject]@{ Name = 'arm64'; Label = 'ARM64'; Target = 'aarch64-pc-windows-msvc' } }
}
$buildLinux = -not $SkipLinux

# --- 1. system -----------------------------------------------------------------
Write-Title "1/5 $($Icons.Search) Checking your PC"
$hostArch = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture
$os = Get-CimInstance Win32_OperatingSystem
Write-Ok "$($os.Caption) ($hostArch)"
$drive = Get-PSDrive -Name (Split-Path -Qualifier $repo).TrimEnd(':') -ErrorAction SilentlyContinue
if ($drive -and $drive.Free -lt 15GB) {
    Write-Warn ("Only {0:N0} GB free on {1}: a full build needs about 15 GB." -f ($drive.Free / 1GB), $drive.Name)
} elseif ($drive) {
    Write-Ok ("{0:N0} GB free on drive {1}:" -f ($drive.Free / 1GB), $drive.Name)
}
$cargoToml = Get-Content -Raw (Join-Path $repo 'Cargo.toml')
$version = [regex]::Match($cargoToml, '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"').Groups[1].Value
$msrv = [regex]::Match($cargoToml, '(?m)^rust-version\s*=\s*"([^"]+)"').Groups[1].Value
if (-not $version) { Exit-WithError 'Could not read the version from Cargo.toml.' }
Write-Ok "Ferrocord $version, profile '$profileName'"

# --- 2. tools ----------------------------------------------------------------------
if ($archs.Count -gt 0) {
    Write-Title "2/5 $($Icons.Tools) Checking the Windows build tools"

    # Rust
    if (-not (Test-Tool cargo) -or -not (Test-Tool rustup)) {
        Write-Warn 'Rust is not installed'
        if (Confirm-Action 'Install Rust (rustup) now?') {
            Install-WithWinget 'Rustlang.Rustup' 'Rust'
            & rustup toolchain install stable --profile minimal
            & rustup default stable
        } else { Exit-WithError 'Cannot build without Rust.' 'Get it from https://rustup.rs' }
    }
    $rustc = (& rustc --version 2>$null) -replace '^rustc (\S+).*$', '$1'
    if (-not $rustc -or [version]($rustc -replace '-.*$', '') -lt [version]$msrv) {
        Write-Warn "Rust $rustc is older than the required $msrv"
        if (Confirm-Action 'Update Rust now?') {
            & rustup toolchain install stable --profile minimal
            & rustup default stable
            $rustc = (& rustc --version) -replace '^rustc (\S+).*$', '$1'
        } else { Exit-WithError "Ferrocord needs Rust $msrv or newer." 'Run: rustup update' }
    }
    Write-Ok "Rust $rustc"

    # Visual Studio C++ build tools (compiler, linker, Windows SDK for the icon resource)
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    $components = @('Microsoft.VisualStudio.Component.VC.Tools.x86.x64')
    if ($archs.Name -contains 'arm64') { $components += 'Microsoft.VisualStudio.Component.VC.Tools.ARM64' }
    function Find-VisualStudio {
        if (-not (Test-Path $vswhere)) { return $null }
        $path = & $vswhere -latest -products '*' -requires $components -property installationPath
        if ($path) { return @($path)[0] } else { return $null }
    }
    $vs = Find-VisualStudio
    if (-not $vs) {
        $existing = if (Test-Path $vswhere) { @(& $vswhere -latest -products '*' -property installationPath)[0] } else { $null }
        $labels = @{ 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64' = 'x64'; 'Microsoft.VisualStudio.Component.VC.Tools.ARM64' = 'ARM64' }
        $missing = @($components | Where-Object { -not (& $vswhere -latest -products '*' -requires $_ -property installationPath) } | ForEach-Object { $labels[$_] })
        $what = if ($existing) { "Your Visual Studio is missing the C++ compiler for $($missing -join ' and ')" } else { 'The Visual Studio C++ Build Tools are not installed' }
        Write-Warn $what
        Write-Hint 'They provide the C/C++ compiler, the linker and the Windows SDK (a few GB).'
        if (Confirm-Action 'Install them now? (a Visual Studio Installer window shows the progress)') {
            $adds = @('--add', 'Microsoft.VisualStudio.Workload.VCTools', '--includeRecommended') + ($components | ForEach-Object { '--add', $_ })
            if ($existing) {
                $setup = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\setup.exe"
                Write-Step 'Adding the C++ components to Visual Studio (Windows will ask for permission)...'
                $p = Start-Process $setup -ArgumentList (@('modify', '--installPath', "`"$existing`"", '--passive', '--norestart') + $adds) -Verb RunAs -Wait -PassThru
                if ($p.ExitCode -notin 0, 3010) { Exit-WithError "The Visual Studio Installer failed (exit code $($p.ExitCode))." }
            } else {
                Install-WithWinget 'Microsoft.VisualStudio.2022.BuildTools' 'Visual Studio Build Tools' @('--override', ((@('--wait', '--passive', '--norestart') + $adds) -join ' '))
            }
            $vs = Find-VisualStudio
            if (-not $vs) { Exit-WithError 'The C++ build tools still cannot be found.' 'Restart Windows and run this script again.' }
        } else { Exit-WithError 'Cannot build without the C++ build tools.' 'Get "Build Tools for Visual Studio" with the "Desktop development with C++" workload.' }
    }
    Write-Ok "C++ build tools ($vs)"

    # CMake (builds the Opus audio codec)
    if (-not (Test-Tool cmake)) {
        $bundled = Join-Path $vs 'Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin'
        if (Test-Path (Join-Path $bundled 'cmake.exe')) {
            $env:Path = "$bundled;$env:Path"
        } elseif (Confirm-Action 'CMake is missing. Install it now?') {
            Install-WithWinget 'Kitware.CMake' 'CMake'
        } else { Exit-WithError 'Cannot build without CMake.' 'Get it from https://cmake.org/download/' }
    }
    Write-Ok ((& cmake --version | Select-Object -First 1) -replace 'cmake version', 'CMake')

    # LLVM/Clang: the TLS library's ARM64 assembly needs it
    if ($archs.Name -contains 'arm64') {
        if (-not (Test-Tool clang-cl)) {
            Write-Warn 'LLVM (clang) is missing; the ARM64 build of the TLS library needs it'
            if (Confirm-Action 'Install LLVM now?') { Install-WithWinget 'LLVM.LLVM' 'LLVM' } else { Write-Warn 'Continuing without LLVM; the ARM64 build will probably fail.' }
        }
        if (Test-Tool clang-cl) { Write-Ok ((& clang-cl --version | Select-Object -First 1) -replace '\s*\(.*$', '') }
    }

    # Rust targets
    $installed = @(& rustup target list --installed)
    foreach ($a in $archs) {
        if ($installed -notcontains $a.Target) {
            Write-Step "Adding the Rust target for $($a.Label)..."
            & rustup target add $a.Target
            if ($LASTEXITCODE -ne 0) { Exit-WithError "Could not add the Rust target $($a.Target)." }
        }
    }
    Write-Ok ('Rust targets: ' + (($archs | ForEach-Object { $_.Target }) -join ', '))

    # .NET SDK + WiX Toolset 5 (makes the .msi)
    $sdks = if (Test-Tool dotnet) { @(& dotnet --list-sdks 2>$null) } else { @() }
    if ($sdks.Count -eq 0) {
        Write-Warn 'The .NET SDK is missing (needed to install the WiX Toolset, which makes the .msi)'
        if (Confirm-Action 'Install the .NET 8 SDK now?') { Install-WithWinget 'Microsoft.DotNet.SDK.8' '.NET 8 SDK' } else { Exit-WithError 'Cannot make the .msi without the .NET SDK.' }
    }
    Write-Ok ".NET SDK $((@(& dotnet --list-sdks) | Select-Object -Last 1) -replace '\s*\[.*$', '')"

    $env:DOTNET_ROLL_FORWARD = 'Major'   # WiX 5 targets .NET 6; run it on the newer runtime
    $wixVersion = '5.0.2'
    $wixNow = if (Test-Tool wix) { (& wix --version 2>$null) -replace '\+.*$', '' } else { '' }
    if ($wixNow -ne $wixVersion) {
        Write-Warn $(if ($wixNow) { "WiX Toolset $wixNow is installed, but Ferrocord uses $wixVersion" } else { 'The WiX Toolset is missing' })
        if (Confirm-Action "Install WiX Toolset $wixVersion now? (a .NET tool in your user profile)") {
            $verb = if ($wixNow) { 'update' } else { 'install' }
            & dotnet tool $verb --global wix --version $wixVersion
            if ($LASTEXITCODE -ne 0) { Exit-WithError 'Installing the WiX Toolset failed.' }
            Import-SystemPath
        } else { Exit-WithError 'Cannot make the .msi without the WiX Toolset.' }
    }
    foreach ($ext in 'WixToolset.UI.wixext', 'WixToolset.Util.wixext') {
        if (-not (Test-Path (Join-Path $HOME ".wix\extensions\$ext\$wixVersion"))) {
            Write-Step "Adding the WiX extension $ext..."
            & wix extension add --global "$ext/$wixVersion"
            if ($LASTEXITCODE -ne 0) { Exit-WithError "Adding the WiX extension $ext failed." }
        }
    }
    Write-Ok "WiX Toolset $wixVersion with the UI and Util extensions"
}

# --- 3 + 4. build and package Windows ----------------------------------------------
$results = [ordered]@{}
foreach ($a in $archs) {
    Write-Title "3/5 $($Icons.Hammer) Building Ferrocord for Windows $($a.Label)"
    if (($a.Name -eq 'x86_64' -and "$hostArch" -ne 'X64') -or ($a.Name -eq 'arm64' -and "$hostArch" -ne 'Arm64')) {
        Write-Hint "Cross-compiling from $hostArch."
    }
    Write-Hint 'The first build downloads and compiles everything: expect 5-20 minutes.'
    $t0 = Get-Date
    Push-Location $repo
    & cargo build --locked --profile $profileName --target $a.Target -p ferrocord
    $code = $LASTEXITCODE
    Pop-Location
    if ($code -ne 0) {
        Write-Fail "The $($a.Label) build failed. Scroll up for the first 'error:' line."
        $results["Windows $($a.Label)"] = $false
        continue
    }
    Write-Ok ("Built in {0:N0} s" -f ((Get-Date) - $t0).TotalSeconds)

    Write-Title "4/5 $($Icons.Box) Making the $($a.Label) .msi and .zip"
    try {
        & (Join-Path $repo 'packaging\windows\package.ps1') -Exe (Join-Path $repo "target\$($a.Target)\$profileName\ferrocord.exe") `
            -Version $version -Arch $a.Name -OutDir $OutDir
        $results["Windows $($a.Label)"] = $true
    } catch {
        Write-Fail "Packaging failed: $_"
        $results["Windows $($a.Label)"] = $false
    }
}

# --- 5. Linux through WSL2 ----------------------------------------------------------
# Returns the wsl.exe arguments for the Linux build, or $null when WSL first needs setting up.
function Get-LinuxBuildCommand {
    Write-Title "5/5 $($Icons.Penguin) Building the Linux packages with WSL2"
    $env:WSL_UTF8 = '1'
    & wsl.exe --status *> $null
    if ($LASTEXITCODE -ne 0) {
        Write-Warn 'WSL2 is not installed'
        if (Confirm-Action 'Install WSL2 with Ubuntu now? (Windows will ask for permission and may need a restart)') {
            Start-Process wsl.exe -ArgumentList '--install', '-d', 'Ubuntu-24.04' -Wait
            Write-Warn 'When Ubuntu has finished setting up (choose a user name and password, restart if asked),'
            Write-Hint 'run this script again with -LinuxOnly to build the Linux packages.'
        }
        return $null
    }
    $all = @(& wsl.exe --list --quiet | ForEach-Object { ($_ -replace "`0", '').Trim() } | Where-Object { $_ -and $_ -notmatch '^docker-desktop' })
    $name = if ($Distro) { $Distro } else { $all | Where-Object { $_ -match 'Ubuntu|Debian' } | Select-Object -First 1 }
    if (-not $name) {
        if ($all.Count -gt 0) { Write-Warn "No Ubuntu or Debian distribution found (you have: $($all -join ', '))." } else { Write-Warn 'WSL is installed but has no Linux distribution yet.' }
        if (Confirm-Action 'Install Ubuntu for WSL now?') {
            Start-Process wsl.exe -ArgumentList '--install', '-d', 'Ubuntu-24.04' -Wait
            Write-Warn 'When Ubuntu has finished setting up (choose a user name and password),'
            Write-Hint 'run this script again with -LinuxOnly to build the Linux packages.'
        }
        return $null
    }
    Write-Ok "Using WSL distribution '$name'"
    $toLinux = { param($p) (& wsl.exe -d $name -- wslpath -a ($p -replace '\\', '/')).Trim() }
    $linuxRepo = & $toLinux $repo
    $linuxOut = & $toLinux $OutDir
    $flags = @('--out', $linuxOut)
    if ($Yes) { $flags += '--yes' }
    if ($Quick) { $flags += '--quick' }
    return @('-d', $name, '--', 'bash', "$linuxRepo/scripts/build-linux.sh") + $flags
}
if ($buildLinux) {
    $wslArgs = Get-LinuxBuildCommand
    if ($wslArgs) {
        # Run at script level so the Linux build talks to this console directly (colours, prompts).
        & wsl.exe @wslArgs
        $results['Linux (deb, rpm, AppImage, tar.gz)'] = ($LASTEXITCODE -eq 0)
    } else {
        $results['Linux (deb, rpm, AppImage, tar.gz)'] = $null
    }
}

# --- summary ------------------------------------------------------------------------
Write-Title "$($Icons.Flag) Summary"
foreach ($k in $results.Keys) {
    if ($results[$k] -eq $true) { Write-Ok $k }
    elseif ($null -eq $results[$k]) { Write-Warn "$k (skipped: finish the WSL setup first)" }
    else { Write-Fail $k }
}
Write-Host ''
Write-Host "$($Icons.Folder) $OutDir" -ForegroundColor White
$files = @(Get-ChildItem $OutDir -File | Where-Object { $_.Name -like "*$version*" } | Sort-Object Name)
foreach ($f in $files) { Write-Host ("   {0}  " -f $f.Name) -NoNewline; Write-Host ("({0:N1} MB)" -f ($f.Length / 1MB)) -ForegroundColor DarkGray }
Write-Hint 'macOS disk images are built on a Mac with scripts/build-macos.sh.'
Write-Host ''
$elapsed = (Get-Date) - $started
$time = '{0}m {1:00}s' -f [int][math]::Floor($elapsed.TotalMinutes), $elapsed.Seconds
if (@($results.Values | Where-Object { $_ -eq $false }).Count -eq 0) {
    Write-Host "$($Icons.Party) All done in $time!" -ForegroundColor Green
    if ($files.Count -gt 0) { Invoke-Item $OutDir }
    exit 0
} else {
    Write-Host "$($Icons.Sad) Finished with errors after $time." -ForegroundColor Red
    exit 1
}
