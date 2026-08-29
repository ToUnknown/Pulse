# Pulse Virtual Microphone

This directory contains Pulse's x64 Windows virtual microphone. Its kernel surface is kept to
two PortCls subdevices: a topology filter and a WaveRT capture filter. The topology connects one
microphone pin to one bridge pin. The WaveRT filter connects that bridge to one 48 kHz, mono,
PCM16 capture pin. The INF registers audio, capture, realtime, and topology interfaces only; it
does not register a render interface.

`GUID_DEVINTERFACE_PULSE_VIRTUAL_MIC` is a separate private control interface. Protocol version 1
uses buffered IOCTLs and fixed 972-byte audio packets: three little-endian `u32` fields followed
by 480 little-endian `i16` samples. The driver accepts one writer, clears its one-second nonpaged
ring on resets and writer cleanup, drops the oldest data on overflow, and supplies zeros on
underflow. Capture-pin transitions into and out of `KSSTATE_RUN` update the consumer count and
state generation returned by `GET_CONSUMER_STATE`.

`PulseDriverInstaller.exe` is a console helper with an administrator manifest. It creates and
removes the `ROOT\PulseVirtualMic` devnode with SetupAPI/NewDev, and removes the matching Driver
Store package with supported SetupAPI/NewDev calls. It has no GUI and does not depend on DevCon.
The legacy removal operation requires both VB-Audio's hardware ID and service name; Pulse invokes
it only when its existing ownership marker says Pulse installed that package.

Build both x64 configurations from the repository root:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-windows-driver.ps1
```

Pass `-TestCertificateThumbprint <thumbprint>` to sign development packages with an existing
certificate in the Local Machine personal store. The script does not create or trust a
certificate, enable TESTSIGNING, or change Secure Boot, BitLocker, or boot configuration. Pass a
Microsoft-signed INF/SYS/CAT directory with `-SignedPackagePath`, plus
`-RequireMicrosoftSignature`, when staging a distributable Release package.

Generated build and package files are written only below
`src-tauri/target/audio-driver/windows/x64/<Configuration>`.

For local installation tests, create and trust a non-exportable test certificate from an elevated
PowerShell session, then rebuild both packages with that certificate:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/setup-windows-driver-test-certificate.ps1
$thumbprint = Get-Content src-tauri/target/audio-driver/test-signing/thumbprint.txt
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/build-windows-driver.ps1 -Configuration Both -TestCertificateThumbprint $thumbprint
```

Debug Pulse builds read the staged Release package directly from `src-tauri/target`, so restart
`pnpm tauri dev` after building. Windows must be in TESTSIGNING mode before it can load this
development certificate. Enabling TESTSIGNING changes BCD and requires a reboot; Pulse does not
perform that change automatically.

The implementation follows Microsoft's current
[SysVAD sample](https://github.com/microsoft/Windows-driver-samples/tree/main/audio/sysvad) for
PortCls/WaveRT structure and Microsoft's documented SetupAPI/NewDev installation model. It does
not retain SysVAD's render, loopback, offload, APO, tone, keyword, or multi-endpoint features.

