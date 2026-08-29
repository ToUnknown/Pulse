[CmdletBinding()]
param(
    [ValidateSet('Debug', 'Release', 'Both')]
    [string]$Configuration = 'Both',
    [string]$TestCertificateThumbprint,
    [string]$SignedPackagePath,
    [switch]$RequireMicrosoftSignature
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$driverRoot = Join-Path $repoRoot 'src-tauri\audio-driver\windows'
$targetRoot = [IO.Path]::GetFullPath((Join-Path $repoRoot 'src-tauri\target\audio-driver\windows'))
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'

if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) {
    throw 'Visual Studio Installer could not be found. Install Visual Studio 2022 Build Tools with Desktop development with C++.'
}

$visualStudio = & $vswhere -latest -products * -requires Microsoft.Component.MSBuild -property installationPath
if (-not $visualStudio) {
    throw 'Visual Studio 2022 with MSBuild could not be found.'
}
$msbuild = Join-Path $visualStudio 'MSBuild\Current\Bin\amd64\MSBuild.exe'
if (-not (Test-Path -LiteralPath $msbuild -PathType Leaf)) {
    $msbuild = Join-Path $visualStudio 'MSBuild\Current\Bin\MSBuild.exe'
}
if (-not (Test-Path -LiteralPath $msbuild -PathType Leaf)) {
    throw "MSBuild is missing from $visualStudio."
}

$kitsRoot = (Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots' -ErrorAction SilentlyContinue).KitsRoot10
if (-not $kitsRoot) {
    throw 'Windows Kits 10 could not be found. Install the Windows 11 SDK and WDK.'
}

$wdkVersions = Get-ChildItem -LiteralPath (Join-Path $kitsRoot 'Include') -Directory -ErrorAction SilentlyContinue |
    Where-Object {
        (Test-Path -LiteralPath (Join-Path $_.FullName 'km\wdm.h') -PathType Leaf) -and
        (Test-Path -LiteralPath (Join-Path $_.FullName 'km\portcls.h') -PathType Leaf) -and
        (Test-Path -LiteralPath (Join-Path $kitsRoot "Lib\$($_.Name)\km\x64\portcls.lib") -PathType Leaf)
    } |
    Sort-Object { [version]$_.Name } -Descending

if (-not $wdkVersions) {
    throw "The Windows Driver Kit is incomplete at $kitsRoot. Install the Windows 11 WDK so km\wdm.h, km\portcls.h, x64\portcls.lib, Inf2Cat.exe, and the WindowsKernelModeDriver10.0 MSBuild toolset are present."
}
$wdkVersion = $wdkVersions[0].Name
$inf2Cat = Join-Path $kitsRoot "bin\$wdkVersion\x86\Inf2Cat.exe"
if (-not (Test-Path -LiteralPath $inf2Cat -PathType Leaf)) {
    $inf2Cat = Join-Path $kitsRoot "bin\$wdkVersion\x64\Inf2Cat.exe"
}
if (-not (Test-Path -LiteralPath $inf2Cat -PathType Leaf)) {
    throw "Inf2Cat.exe is missing for WDK $wdkVersion. Repair the Windows Driver Kit installation."
}
$signTool = Join-Path $kitsRoot "bin\$wdkVersion\x64\signtool.exe"
if (-not (Test-Path -LiteralPath $signTool -PathType Leaf)) {
    throw "signtool.exe is missing for Windows Kit $wdkVersion."
}

$driverToolset = Get-ChildItem -LiteralPath (Join-Path $visualStudio 'MSBuild\Microsoft\VC') -Directory -Filter 'v*' |
    Sort-Object Name -Descending |
    ForEach-Object {
        Join-Path $_.FullName 'Platforms\x64\PlatformToolsets\WindowsKernelModeDriver10.0\Toolset.targets'
    } |
    Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
    Select-Object -First 1
if (-not $driverToolset) {
    throw "The WDK Visual Studio integration is missing under $visualStudio. Repair the WDK after Visual Studio is installed."
}

$configurations = if ($Configuration -eq 'Both') { @('Debug', 'Release') } else { @($Configuration) }
foreach ($current in $configurations) {
    & $msbuild (Join-Path $driverRoot 'PulseVirtualMic.sln') /m /t:Build "/p:Configuration=$current" /p:Platform=x64 "/p:WindowsTargetPlatformVersion=$wdkVersion" /p:SignMode=Off /p:EnableTestSign=false /p:EnableInf2cat=false /v:minimal
    if ($LASTEXITCODE -ne 0) {
        throw "$current x64 Pulse driver build failed with exit code $LASTEXITCODE."
    }

    $configurationRoot = [IO.Path]::GetFullPath((Join-Path $targetRoot "x64\$current"))
    $package = [IO.Path]::GetFullPath((Join-Path $configurationRoot 'package'))
    if (-not $package.StartsWith($targetRoot, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to stage outside the driver target directory: $package"
    }
    if (Test-Path -LiteralPath $package) {
        Remove-Item -LiteralPath $package -Recurse -Force
    }
    New-Item -ItemType Directory -Path $package | Out-Null

    $build = Join-Path $configurationRoot 'build'
    $installer = Join-Path $configurationRoot 'installer\PulseDriverInstaller.exe'
    foreach ($file in @(
        (Join-Path $driverRoot 'driver\PulseVirtualMic.inf'),
        (Join-Path $build 'PulseVirtualMic.sys'),
        $installer
    )) {
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) {
            throw "The $current build did not produce $file."
        }
        Copy-Item -LiteralPath $file -Destination $package
    }

    if ($SignedPackagePath) {
        $signed = [IO.Path]::GetFullPath($SignedPackagePath)
        foreach ($name in @('PulseVirtualMic.inf', 'PulseVirtualMic.sys', 'PulseVirtualMic.cat')) {
            $source = Join-Path $signed $name
            if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
                throw "The signed package is missing $source."
            }
            Copy-Item -LiteralPath $source -Destination (Join-Path $package $name) -Force
        }
    }
    else {
        if ($TestCertificateThumbprint) {
            & $signTool sign /v /fd SHA256 /ph /sha1 $TestCertificateThumbprint /s My /sm (Join-Path $package 'PulseVirtualMic.sys')
            if ($LASTEXITCODE -ne 0) {
                throw "Test-signing PulseVirtualMic.sys failed with exit code $LASTEXITCODE."
            }
        }

        if ($TestCertificateThumbprint) {
            & $signTool verify /v /pa /ph (Join-Path $package 'PulseVirtualMic.sys')
            if ($LASTEXITCODE -ne 0) {
                throw "PulseVirtualMic.sys does not contain a valid embedded test signature with page hashes."
            }
        }
        & $inf2Cat "/driver:$package" /os:10_X64
        if ($LASTEXITCODE -ne 0) {
            throw "Inf2Cat failed for the $current package with exit code $LASTEXITCODE."
        }
        if ($TestCertificateThumbprint) {
            & $signTool sign /v /fd SHA256 /sha1 $TestCertificateThumbprint /s My /sm (Join-Path $package 'PulseVirtualMic.cat')
            if ($LASTEXITCODE -ne 0) {
                throw "Test-signing PulseVirtualMic.cat failed with exit code $LASTEXITCODE."
            }
        }
    }

    if ($RequireMicrosoftSignature) {
        $catalog = Join-Path $package 'PulseVirtualMic.cat'
        $driver = Join-Path $package 'PulseVirtualMic.sys'
        & $signTool verify /v /kp $catalog
        if ($LASTEXITCODE -ne 0) {
            throw "PulseVirtualMic.cat is not trusted by Windows kernel-mode code-signing policy. Supply a Microsoft-signed package with -SignedPackagePath."
        }
        & $signTool verify /v /kp /c $catalog $driver
        if ($LASTEXITCODE -ne 0) {
            throw "PulseVirtualMic.sys is not covered by the trusted PulseVirtualMic.cat. Supply a complete Microsoft-signed package with -SignedPackagePath."
        }
    }

    $hashLines = Get-ChildItem -LiteralPath $package -File |
        Where-Object Name -ne 'SHA256SUMS.txt' |
        Sort-Object Name |
        ForEach-Object {
        $hash = Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256
        "{0}  {1}" -f $hash.Hash.ToLowerInvariant(), $_.Name
    }
    $hashLines | Out-File -LiteralPath (Join-Path $package 'SHA256SUMS.txt') -Encoding ascii

    Write-Host "Staged $current x64 package at $package"
}
