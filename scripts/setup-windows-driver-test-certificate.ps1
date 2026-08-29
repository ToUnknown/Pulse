[CmdletBinding()]
param(
    [switch]$DisposableVmAcknowledged
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if (-not $DisposableVmAcknowledged) {
    throw 'Pulse test drivers must only be installed in a disposable Windows virtual machine. Re-run with -DisposableVmAcknowledged inside that VM.'
}

$principal = [Security.Principal.WindowsPrincipal]::new(
    [Security.Principal.WindowsIdentity]::GetCurrent()
)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this script from an elevated PowerShell session.'
}

$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$targetRoot = [IO.Path]::GetFullPath((Join-Path $repoRoot 'src-tauri\target\audio-driver'))
$outputDirectory = [IO.Path]::GetFullPath((Join-Path $targetRoot 'test-signing'))
if (-not $outputDirectory.StartsWith($targetRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to write test-signing files outside $targetRoot."
}
New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null

$subject = 'CN=Pulse Virtual Microphone Test'
$minimumExpiry = (Get-Date).AddDays(30)
$certificate = Get-ChildItem -LiteralPath 'Cert:\LocalMachine\My' |
    Where-Object {
        $_.Subject -eq $subject -and
        $_.HasPrivateKey -and
        $_.NotAfter -gt $minimumExpiry
    } |
    Sort-Object NotAfter -Descending |
    Select-Object -First 1

if (-not $certificate) {
    $certificate = New-SelfSignedCertificate `
        -Type CodeSigningCert `
        -Subject $subject `
        -FriendlyName 'Pulse Virtual Microphone test signing' `
        -CertStoreLocation 'Cert:\LocalMachine\My' `
        -KeyAlgorithm RSA `
        -KeyLength 3072 `
        -HashAlgorithm SHA256 `
        -KeyExportPolicy NonExportable `
        -NotAfter (Get-Date).AddYears(2)
}

$cerPath = Join-Path $outputDirectory 'PulseVirtualMicTest.cer'
Export-Certificate -Cert $certificate -FilePath $cerPath -Force | Out-Null
foreach ($store in @('Root', 'TrustedPublisher')) {
    $trustedPath = "Cert:\LocalMachine\$store\$($certificate.Thumbprint)"
    if (-not (Test-Path -LiteralPath $trustedPath)) {
        Import-Certificate -FilePath $cerPath -CertStoreLocation "Cert:\LocalMachine\$store" |
            Out-Null
    }
}

$thumbprintPath = Join-Path $outputDirectory 'thumbprint.txt'
$certificate.Thumbprint | Out-File -LiteralPath $thumbprintPath -Encoding ascii -NoNewline

[ordered]@{
    status = 'ok'
    subject = $certificate.Subject
    thumbprint = $certificate.Thumbprint
    expires = $certificate.NotAfter.ToUniversalTime().ToString('o')
    certificate = $cerPath
    thumbprintFile = $thumbprintPath
} | ConvertTo-Json -Compress
