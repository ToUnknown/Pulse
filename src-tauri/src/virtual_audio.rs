use std::{fs, io, path::PathBuf, process::Command};

#[cfg(target_os = "windows")]
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use cpal::traits::{DeviceTrait, HostTrait};
use serde::{Deserialize, Serialize};
#[cfg(target_os = "windows")]
use std::path::Path;
#[cfg(target_os = "windows")]
use std::{ffi::OsString, mem, os::windows::ffi::OsStrExt};
use tauri::{path::BaseDirectory, AppHandle, Manager};
#[cfg(target_os = "windows")]
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_CANCELLED, WAIT_OBJECT_0},
    System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE},
    UI::{
        Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW},
        WindowsAndMessaging::SW_HIDE,
    },
};

const OWNER_MARKER: &str = "pulse-installed-vb-cable";
#[cfg(target_os = "windows")]
const WINDOWS_PACKAGE: &str = "resources/virtual-audio/windows/x64";
#[cfg(target_os = "windows")]
const WINDOWS_PACKAGE_FILES: [&str; 5] = [
    "PulseVirtualMic.inf",
    "PulseVirtualMic.sys",
    "PulseVirtualMic.cat",
    "PulseDriverInstaller.exe",
    "MICROSOFT_SIGNED",
];
#[cfg(target_os = "windows")]
const WINDOWS_SIGNED_MARKER: &str =
    "Microsoft kernel-mode signature verified by scripts/build-windows-driver.ps1.";
#[cfg(target_os = "windows")]
const WINDOWS_INSTALLER: &str = "PulseDriverInstaller.exe";
#[cfg(target_os = "macos")]
const MACOS_DRIVER: &str = "target/pulse-audio-driver/Pulse.driver";
#[cfg(target_os = "macos")]
const INSTALLED_MACOS_DRIVER: &str = "/Library/Audio/Plug-Ins/HAL/Pulse.driver";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LifecycleResult {
    pub(crate) restart_required: bool,
}

#[cfg(target_os = "macos")]
pub(crate) fn enable(app: &AppHandle) -> Result<LifecycleResult, String> {
    let remove_legacy_vb_cable = owned_provider(app)? == Some(OwnedProvider::LegacyVbCable);
    destroy_legacy_pulse_aggregate()?;
    let driver = bundled_macos_driver(app)?;
    install_macos_driver(&driver, remove_legacy_vb_cable)?;
    mark_provider_owned(app, OwnedProvider::PulseDriver)?;

    Ok(LifecycleResult {
        restart_required: !wait_for_pulse_state(true),
    })
}

#[cfg(target_os = "windows")]
pub(crate) fn enable(app: &AppHandle) -> Result<LifecycleResult, String> {
    let package = bundled_windows_package(app)?;
    let previous_owner = owned_provider(app)?;
    let legacy_was_renamed = legacy_capture_endpoint_renamed()?;
    if legacy_was_renamed {
        restore_legacy_windows_capture_endpoint()?;
    }

    let pulse_driver_was_present = crate::windows_audio::interface_available();
    let mut restart_required = false;
    if !pulse_driver_was_present {
        let install = run_windows_installer(&package, "install")?;
        restart_required |= install.restart_required;
    }

    if !wait_for_windows_pulse_state(true) {
        let rollback = (!pulse_driver_was_present)
            .then(|| run_windows_installer(&package, "remove"))
            .transpose();
        return Err(match rollback {
            Ok(_) => "PulseVirtualMic was installed, but Windows did not publish its Pulse recording endpoint; the incomplete installation was removed".to_string(),
            Err(rollback_error) => format!(
                "PulseVirtualMic was installed, but Windows did not publish its Pulse recording endpoint; rollback also failed: {rollback_error}"
            ),
        });
    }

    if previous_owner == Some(OwnedProvider::LegacyVbCable) && legacy_windows_provider_present() {
        match run_windows_installer(&package, "remove-legacy") {
            Ok(removal) => restart_required |= removal.restart_required,
            Err(legacy_error) => {
                if pulse_driver_was_present {
                    return Err(format!(
                        "the new Pulse input was verified, but the Pulse-owned legacy VB-CABLE package could not be removed: {legacy_error}; the existing PulseVirtualMic installation was left intact"
                    ));
                }
                return Err(match run_windows_installer(&package, "remove") {
                    Ok(_) => format!(
                        "the new Pulse input was verified, but the Pulse-owned legacy VB-CABLE package could not be removed: {legacy_error}; the new driver was rolled back"
                    ),
                    Err(rollback_error) => format!(
                        "the new Pulse input was verified, but the Pulse-owned legacy VB-CABLE package could not be removed: {legacy_error}; rollback also failed: {rollback_error}"
                    ),
                });
            }
        }
    }

    if !pulse_driver_was_present || previous_owner == Some(OwnedProvider::LegacyVbCable) {
        mark_provider_owned(app, OwnedProvider::PulseDriver)?;
    }

    Ok(LifecycleResult { restart_required })
}

#[cfg(target_os = "macos")]
pub(crate) fn disable(app: &AppHandle) -> Result<LifecycleResult, String> {
    destroy_legacy_pulse_aggregate()?;
    if let Some(provider) = owned_provider(app)? {
        remove_macos_driver(provider == OwnedProvider::LegacyVbCable)?;
        clear_provider_owned(app)?;
    }

    Ok(LifecycleResult {
        restart_required: !wait_for_pulse_state(false)
            || std::path::Path::new(INSTALLED_MACOS_DRIVER).exists(),
    })
}

#[cfg(target_os = "windows")]
pub(crate) fn disable(app: &AppHandle) -> Result<LifecycleResult, String> {
    let package = bundled_windows_package(app)?;
    let owner = owned_provider(app)?;
    let restart_required = match owner {
        Some(OwnedProvider::PulseDriver) => {
            let removal = run_windows_installer(&package, "remove")?;
            if !wait_for_windows_pulse_state(false) {
                return Err("Windows did not remove the Pulse recording endpoint and private driver interface".to_string());
            }
            clear_provider_owned(app)?;
            removal.restart_required
        }
        Some(OwnedProvider::LegacyVbCable) => {
            let removal = run_windows_installer(&package, "remove-legacy")?;
            clear_provider_owned(app)?;
            removal.restart_required
        }
        None => {
            if legacy_capture_endpoint_renamed()? {
                restore_legacy_windows_capture_endpoint()?;
            }
            false
        }
    };

    if crate::windows_audio::interface_available() && owner != Some(OwnedProvider::PulseDriver) {
        return Ok(LifecycleResult { restart_required });
    }

    Ok(LifecycleResult { restart_required })
}

#[cfg(target_os = "macos")]
fn bundled_macos_driver(app: &AppHandle) -> Result<PathBuf, String> {
    let path = app
        .path()
        .resolve(MACOS_DRIVER, BaseDirectory::Resource)
        .map_err(|error| format!("could not locate the bundled Pulse audio driver: {error}"))?;
    path.is_dir()
        .then_some(path)
        .ok_or_else(|| "the bundled Pulse audio driver is missing".to_string())
}

#[cfg(target_os = "windows")]
fn bundled_windows_package(app: &AppHandle) -> Result<PathBuf, String> {
    let bundled = app
        .path()
        .resolve(WINDOWS_PACKAGE, BaseDirectory::Resource)
        .map_err(|error| format!("could not locate the bundled Pulse audio component: {error}"))?;
    if windows_package_missing_files(&bundled).is_empty() {
        return Ok(bundled);
    }

    #[cfg(debug_assertions)]
    {
        let development = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/audio-driver/windows/x64/Release/package");
        let missing = windows_package_missing_files(&development);
        if missing.is_empty() {
            return Ok(development);
        }
        Err(format!(
            "Pulse will not install a test-signed kernel driver; stage a Microsoft-signed PulseVirtualMic package at {} (missing {}) or test the development package manually in a disposable virtual machine",
            development.display(),
            missing.join(", ")
        ))
    }

    #[cfg(not(debug_assertions))]
    {
        let missing = windows_package_missing_files(&bundled);
        Err(format!(
            "the bundled PulseVirtualMic driver package is incomplete or was not verified as Microsoft-signed (missing {}); reinstall Pulse",
            missing.join(", ")
        ))
    }
}

#[cfg(target_os = "windows")]
fn windows_package_missing_files(package: &Path) -> Vec<&'static str> {
    WINDOWS_PACKAGE_FILES
        .iter()
        .copied()
        .filter(|name| {
            let path = package.join(name);
            if *name == "MICROSOFT_SIGNED" {
                fs::read_to_string(path)
                    .map(|contents| contents.trim() != WINDOWS_SIGNED_MARKER)
                    .unwrap_or(true)
            } else {
                !path.is_file()
            }
        })
        .collect()
}

fn owner_marker(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join(OWNER_MARKER))
        .map_err(|error| error.to_string())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OwnedProvider {
    LegacyVbCable,
    PulseDriver,
}

fn mark_provider_owned(app: &AppHandle, provider: OwnedProvider) -> Result<(), String> {
    let marker = owner_marker(app)?;
    if let Some(parent) = marker.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let value = match provider {
        OwnedProvider::LegacyVbCable => "VB-CABLE\n",
        OwnedProvider::PulseDriver => "PULSE_DRIVER\n",
    };
    fs::write(marker, value).map_err(|error| error.to_string())
}

fn owned_provider(app: &AppHandle) -> Result<Option<OwnedProvider>, String> {
    match fs::read_to_string(owner_marker(app)?) {
        Ok(value) => Ok(Some(parse_owned_provider(&value))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn parse_owned_provider(value: &str) -> OwnedProvider {
    if value.trim() == "PULSE_DRIVER" {
        OwnedProvider::PulseDriver
    } else {
        OwnedProvider::LegacyVbCable
    }
}

fn clear_provider_owned(app: &AppHandle) -> Result<(), String> {
    let marker = owner_marker(app)?;
    match fs::remove_file(marker) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(target_os = "windows")]
fn backing_provider_present() -> bool {
    audio_device_names().iter().any(|name| {
        let name = name.to_ascii_lowercase();
        name != "pulse"
            && [
                "vb-cable",
                "vb cable",
                "cable input",
                "cable output",
                "virtual audio cable",
            ]
            .iter()
            .any(|candidate| name.contains(candidate))
    })
}

fn pulse_input_present() -> bool {
    cpal::default_host()
        .input_devices()
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|device| device.description().ok())
        .any(|description| description.name().eq_ignore_ascii_case("Pulse"))
}

#[cfg(target_os = "macos")]
fn wait_for_pulse_state(expected: bool) -> bool {
    for _ in 0..50 {
        if pulse_input_present() == expected {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    pulse_input_present() == expected
}

#[cfg(target_os = "windows")]
fn wait_for_windows_pulse_state(expected: bool) -> bool {
    for _ in 0..50 {
        let interface_present = crate::windows_audio::interface_available();
        if interface_present == expected && pulse_input_present() == expected {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    crate::windows_audio::interface_available() == expected && pulse_input_present() == expected
}

#[cfg(target_os = "windows")]
fn audio_device_names() -> Vec<String> {
    let host = cpal::default_host();
    let mut names = Vec::new();
    if let Ok(devices) = host.input_devices() {
        names.extend(
            devices
                .filter_map(|device| device.description().ok())
                .map(|description| description.name().to_string()),
        );
    }
    if let Ok(devices) = host.output_devices() {
        names.extend(
            devices
                .filter_map(|device| device.description().ok())
                .map(|description| description.name().to_string()),
        );
    }
    names
}

#[cfg(target_os = "macos")]
fn install_macos_driver(driver: &std::path::Path, remove_legacy: bool) -> Result<(), String> {
    const SCRIPT: &str = r#"on run argv
set sourcePath to item 1 of argv
set removeLegacy to item 2 of argv
set legacyCommand to ""
if removeLegacy is "1" then
    set legacyCommand to "/bin/launchctl bootout system /Library/LaunchDaemons/com.vbaudio.vbcableagent.plist >/dev/null 2>&1 || true; /bin/rm -rf -- /Library/Audio/Plug-Ins/HAL/VBCable.driver; /bin/rm -f -- /Library/LaunchDaemons/com.vbaudio.vbcableagent.plist; "
end if
do shell script (legacyCommand & "/bin/mkdir -p /Library/Audio/Plug-Ins/HAL; /bin/rm -rf -- /Library/Audio/Plug-Ins/HAL/Pulse.driver; /bin/cp -R " & quoted form of sourcePath & " /Library/Audio/Plug-Ins/HAL/Pulse.driver; /usr/sbin/chown -R root:wheel /Library/Audio/Plug-Ins/HAL/Pulse.driver; /usr/bin/killall -9 coreaudiod >/dev/null 2>&1 || true") with administrator privileges
end run"#;

    let status = Command::new("/usr/bin/osascript")
        .args(["-e", SCRIPT])
        .arg(driver)
        .arg(if remove_legacy { "1" } else { "0" })
        .status()
        .map_err(|error| format!("could not install the Pulse audio driver: {error}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| "Pulse audio installation was cancelled".to_string())
}

#[cfg(target_os = "macos")]
fn remove_macos_driver(remove_legacy: bool) -> Result<(), String> {
    const SCRIPT: &str = r#"on run argv
set removeLegacy to item 1 of argv
set legacyCommand to ""
if removeLegacy is "1" then
    set legacyCommand to "/bin/launchctl bootout system /Library/LaunchDaemons/com.vbaudio.vbcableagent.plist >/dev/null 2>&1 || true; /bin/rm -rf -- /Library/Audio/Plug-Ins/HAL/VBCable.driver; /bin/rm -f -- /Library/LaunchDaemons/com.vbaudio.vbcableagent.plist; "
end if
do shell script (legacyCommand & "/bin/rm -rf -- /Library/Audio/Plug-Ins/HAL/Pulse.driver; /usr/bin/killall -9 coreaudiod >/dev/null 2>&1 || true") with administrator privileges
end run"#;

    let status = Command::new("/usr/bin/osascript")
        .args(["-e", SCRIPT])
        .arg(if remove_legacy { "1" } else { "0" })
        .status()
        .map_err(|error| format!("could not remove the Pulse audio driver: {error}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| "Pulse audio removal was cancelled".to_string())
}

#[cfg(target_os = "windows")]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowsInstallerResult {
    ok: bool,
    restart_required: bool,
    error_code: u32,
    #[serde(rename = "devicePresent")]
    _device_present: bool,
    inf_name: String,
    message: String,
}

#[cfg(target_os = "windows")]
fn run_windows_installer(
    package: &Path,
    operation: &str,
) -> Result<WindowsInstallerResult, String> {
    let installer = package.join(WINDOWS_INSTALLER);
    if !installer.is_file() {
        return Err(format!(
            "the Pulse driver installer is missing: {}",
            installer.display()
        ));
    }
    let result_directory = tempfile::Builder::new()
        .prefix("pulse-driver-result-")
        .tempdir()
        .map_err(|error| format!("could not prepare the Pulse driver installer: {error}"))?;
    let result_path = result_directory.path().join("result.json");

    let exit_code = run_windows_installer_elevated(&installer, package, &result_path, operation)?;

    let contents = fs::read_to_string(&result_path).map_err(|error| {
        if exit_code == 0 {
            format!("the Pulse driver installer did not return a result: {error}")
        } else {
            format!(
                "the elevated Pulse driver installer exited with code {exit_code} without returning a result"
            )
        }
    })?;
    let result: WindowsInstallerResult = serde_json::from_str(&contents)
        .map_err(|error| format!("the Pulse driver installer returned invalid status: {error}"))?;
    if !result.ok || exit_code != 0 {
        let details = if result.inf_name.is_empty() {
            String::new()
        } else {
            format!(" ({})", result.inf_name)
        };
        return Err(format!(
            "{} [Windows error {}]{}",
            result.message, result.error_code, details
        ));
    }
    Ok(result)
}

#[cfg(target_os = "windows")]
fn run_windows_installer_elevated(
    installer: &Path,
    package: &Path,
    result_path: &Path,
    operation: &str,
) -> Result<u32, String> {
    let verb = "runas\0".encode_utf16().collect::<Vec<_>>();
    let installer = installer
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut parameters = OsString::from(operation);
    parameters.push(" --package \"");
    parameters.push(package.as_os_str());
    parameters.push("\" --result \"");
    parameters.push(result_path.as_os_str());
    parameters.push("\"");
    let parameters = parameters.encode_wide().chain(Some(0)).collect::<Vec<_>>();

    let mut execute = SHELLEXECUTEINFOW {
        cbSize: mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: verb.as_ptr(),
        lpFile: installer.as_ptr(),
        lpParameters: parameters.as_ptr(),
        nShow: SW_HIDE,
        ..Default::default()
    };

    // ShellExecuteExW is the supported Windows path for requesting UAC elevation. The strings
    // above remain alive until the call returns, and a successful call owns hProcess below.
    if unsafe { ShellExecuteExW(&mut execute) } == 0 {
        let error = std::io::Error::last_os_error();
        return if error.raw_os_error() == Some(ERROR_CANCELLED as i32) {
            Err("Pulse driver installation was cancelled at the Windows UAC prompt".to_string())
        } else {
            Err(format!(
                "could not elevate the Pulse driver installer: {error}"
            ))
        };
    }
    if execute.hProcess.is_null() {
        return Err(
            "Windows elevated the Pulse driver installer without returning a process handle"
                .to_string(),
        );
    }

    let wait = unsafe { WaitForSingleObject(execute.hProcess, INFINITE) };
    if wait != WAIT_OBJECT_0 {
        let error = std::io::Error::last_os_error();
        unsafe {
            CloseHandle(execute.hProcess);
        }
        return Err(format!(
            "could not wait for the elevated Pulse driver installer: {error}"
        ));
    }

    let mut exit_code = 0;
    let exit_result = unsafe { GetExitCodeProcess(execute.hProcess, &mut exit_code) };
    let exit_error = (exit_result == 0).then(std::io::Error::last_os_error);
    unsafe {
        CloseHandle(execute.hProcess);
    }
    if let Some(error) = exit_error {
        return Err(format!(
            "could not read the Pulse driver installer exit code: {error}"
        ));
    }
    Ok(exit_code)
}

#[cfg(target_os = "windows")]
pub(crate) fn run_uninstall_maintenance_if_requested() -> Option<Result<(), String>> {
    let requested = std::env::args_os()
        .nth(1)
        .is_some_and(|argument| argument == "--uninstall-pulse-driver");
    if !requested {
        return None;
    }

    Some((|| {
        let app_data = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| {
                "APPDATA is unavailable; Pulse cannot inspect driver ownership".to_string()
            })?;
        let marker = app_data.join("app.pulse.desktop").join(OWNER_MARKER);
        let owner = match fs::read_to_string(&marker) {
            Ok(value) => parse_owned_provider(&value),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("could not read Pulse driver ownership: {error}")),
        };
        let executable = std::env::current_exe()
            .map_err(|error| format!("could not locate Pulse.exe: {error}"))?;
        let package = executable
            .parent()
            .ok_or_else(|| "Pulse.exe has no installation directory".to_string())?
            .join(WINDOWS_PACKAGE);
        let operation = match owner {
            OwnedProvider::PulseDriver => "remove",
            OwnedProvider::LegacyVbCable => "remove-legacy",
        };
        run_windows_installer(&package, operation)?;
        match fs::remove_file(&marker) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!(
                "the driver was removed, but its ownership marker remains: {error}"
            )),
        }
    })())
}

#[cfg(target_os = "windows")]
fn legacy_windows_provider_present() -> bool {
    backing_provider_present()
        || Command::new("sc.exe")
            .args(["query", "VBAudioVACMME"])
            .output()
            .is_ok_and(|output| output.status.success())
}

#[cfg(target_os = "windows")]
fn legacy_capture_endpoint_renamed() -> Result<bool, String> {
    use winreg::{enums::HKEY_LOCAL_MACHINE, RegKey};

    const CAPTURE: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture";
    const FRIENDLY_NAME: &str = "{a45c254e-df1c-4efd-8020-67d146a850e0},14";
    let local_machine = RegKey::predef(HKEY_LOCAL_MACHINE);
    let capture = match local_machine.open_subkey(CAPTURE) {
        Ok(key) => key,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "could not inspect Windows recording endpoints: {error}"
            ))
        }
    };

    for endpoint in capture.enum_keys().flatten() {
        let Ok(properties) = capture.open_subkey(format!(r"{endpoint}\Properties")) else {
            continue;
        };
        let Ok(name) = properties.get_value::<String, _>(FRIENDLY_NAME) else {
            continue;
        };
        if !name.eq_ignore_ascii_case("Pulse") {
            continue;
        }

        let legacy_backing = properties.enum_values().flatten().any(|(_, value)| {
            let ascii = String::from_utf8_lossy(&value.bytes).to_ascii_lowercase();
            let utf16 = value
                .bytes
                .chunks_exact(2)
                .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
                .collect::<Vec<_>>();
            let utf16 = String::from_utf16_lossy(&utf16).to_ascii_lowercase();
            ["vb-audio", "vbaudio", "cable output", "vbaudiovacwdm"]
                .iter()
                .any(|needle| ascii.contains(needle) || utf16.contains(needle))
        });
        if legacy_backing {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(target_os = "windows")]
fn restore_legacy_windows_capture_endpoint() -> Result<(), String> {
    const RESTORE: &str = r#"
$root = 'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture'
$friendlyName = '{a45c254e-df1c-4efd-8020-67d146a850e0},14'
if (Test-Path -LiteralPath $root) {
  Get-ChildItem -LiteralPath $root | ForEach-Object {
    $properties = Join-Path $_.PSPath 'Properties'
    if (Test-Path -LiteralPath $properties) {
      $current = (Get-ItemProperty -LiteralPath $properties -Name $friendlyName -ErrorAction SilentlyContinue).$friendlyName
      $backing = ((Get-ItemProperty -LiteralPath $properties -ErrorAction SilentlyContinue | Out-String) -match 'VB-Audio|VBAudio|CABLE Output|VBAudioVACWDM')
      if ($current -eq 'Pulse' -and $backing) {
        Set-ItemProperty -LiteralPath $properties -Name $friendlyName -Value 'CABLE Output'
      }
    }
  }
}
"#;
    let encoded = encode_powershell(RESTORE);
    run_elevated_powershell(&encoded)
}

#[cfg(target_os = "windows")]
fn encode_powershell(script: &str) -> String {
    let utf16 = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    BASE64.encode(utf16)
}

#[cfg(target_os = "windows")]
fn run_elevated_powershell(encoded: &str) -> Result<(), String> {
    const LAUNCH: &str = "$p = Start-Process -FilePath 'powershell.exe' -ArgumentList @('-NoProfile','-NonInteractive','-EncodedCommand',$env:PULSE_ELEVATED_COMMAND) -Verb RunAs -Wait -PassThru; exit $p.ExitCode";
    let mut command = Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-NonInteractive", "-Command", LAUNCH])
        .env("PULSE_ELEVATED_COMMAND", encoded);
    let status = command
        .status()
        .map_err(|error| format!("could not start the Pulse audio installer: {error}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| "restoring the legacy VB-CABLE endpoint was cancelled".to_string())
}

#[cfg(target_os = "macos")]
mod legacy_macos_aggregate {
    use std::{ffi::c_void, mem, ptr};

    use core_foundation::{
        base::TCFType,
        string::{CFString, CFStringRef},
    };
    use coreaudio::audio_unit::macos_helpers::get_audio_device_ids;

    const PULSE_UID: &str = "app.pulse.desktop.virtual-input";
    const AUDIO_DEVICE_PROPERTY_UID: u32 = 0x7569_6420;
    const AUDIO_SCOPE_GLOBAL: u32 = 0x676c_6f62;

    #[repr(C)]
    struct AudioObjectPropertyAddress {
        selector: u32,
        scope: u32,
        element: u32,
    }

    #[link(name = "CoreAudio", kind = "framework")]
    extern "C" {
        fn AudioObjectGetPropertyData(
            object_id: u32,
            address: *const AudioObjectPropertyAddress,
            qualifier_size: u32,
            qualifier_data: *const c_void,
            data_size: *mut u32,
            data: *mut c_void,
        ) -> i32;
        fn AudioHardwareDestroyAggregateDevice(device_id: u32) -> i32;
    }

    pub(super) fn destroy() -> Result<(), String> {
        for device_id in get_audio_device_ids().map_err(|error| error.to_string())? {
            if device_uid(device_id).as_deref() == Ok(PULSE_UID) {
                let status = unsafe { AudioHardwareDestroyAggregateDevice(device_id) };
                if status != 0 {
                    return Err(format!(
                        "could not remove the legacy Pulse virtual input ({status})"
                    ));
                }
            }
        }
        Ok(())
    }

    fn device_uid(device_id: u32) -> Result<String, String> {
        let address = AudioObjectPropertyAddress {
            selector: AUDIO_DEVICE_PROPERTY_UID,
            scope: AUDIO_SCOPE_GLOBAL,
            element: 0,
        };
        let mut value: CFStringRef = ptr::null();
        let mut size = mem::size_of::<CFStringRef>() as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(
                device_id,
                &address,
                0,
                ptr::null(),
                &mut size,
                (&mut value as *mut CFStringRef).cast(),
            )
        };
        if status != 0 || value.is_null() {
            return Err(format!(
                "could not read an audio-device identifier ({status})"
            ));
        }
        let value = unsafe { CFString::wrap_under_create_rule(value) };
        Ok(value.to_string())
    }
}

#[cfg(target_os = "macos")]
fn destroy_legacy_pulse_aggregate() -> Result<(), String> {
    legacy_macos_aggregate::destroy()
}

#[cfg(test)]
mod ownership_tests {
    use super::{parse_owned_provider, OwnedProvider};

    #[test]
    fn ownership_marker_distinguishes_new_driver_from_legacy_vb_cable() {
        assert!(matches!(
            parse_owned_provider("PULSE_DRIVER\n"),
            OwnedProvider::PulseDriver
        ));
        assert!(matches!(
            parse_owned_provider("VB-CABLE\n"),
            OwnedProvider::LegacyVbCable
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_inf_registers_one_capture_surface_and_no_render_surface() {
        let inf = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("audio-driver/windows/driver/PulseVirtualMic.inf"),
        )
        .expect("Pulse Windows driver INF");

        assert_eq!(inf.matches("AddInterface=%KSCATEGORY_CAPTURE%").count(), 1);
        assert!(!inf.contains("KSCATEGORY_RENDER"));
        assert!(inf.contains("%DeviceDescription%=PulseVirtualMic,ROOT\\PulseVirtualMic"));
        assert!(inf.contains("EndpointName=\"Pulse\""));
        assert!(inf.contains("PKEY_AudioEndpoint_FormFactor%"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_driver_package_requires_every_installation_file() {
        use super::{windows_package_missing_files, WINDOWS_PACKAGE_FILES, WINDOWS_SIGNED_MARKER};
        use std::fs;

        let directory = tempfile::tempdir().expect("temporary driver package");
        assert_eq!(
            windows_package_missing_files(directory.path()),
            WINDOWS_PACKAGE_FILES
        );

        for name in WINDOWS_PACKAGE_FILES {
            fs::write(directory.path().join(name), []).expect("driver package file");
        }
        assert_eq!(
            windows_package_missing_files(directory.path()),
            ["MICROSOFT_SIGNED"]
        );
        fs::write(
            directory.path().join("MICROSOFT_SIGNED"),
            WINDOWS_SIGNED_MARKER,
        )
        .expect("verified marker");
        assert!(windows_package_missing_files(directory.path()).is_empty());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_wave_rt_timer_lives_for_the_stream_lifetime() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("audio-driver/windows/driver/Stream.cpp"),
        )
        .expect("Pulse WaveRT stream source");
        let set_state = source
            .split("NTSTATUS PulseWaveRTStream::SetState")
            .nth(1)
            .and_then(|tail| tail.split("NTSTATUS PulseWaveRTStream::SetFormat").next())
            .expect("SetState implementation");

        assert_eq!(source.matches("ExAllocateTimer").count(), 1);
        assert_eq!(source.matches("ExDeleteTimer").count(), 1);
        assert!(!set_state.contains("ExAllocateTimer"));
        assert!(!set_state.contains("ExDeleteTimer"));
        assert!(set_state.contains("ExCancelTimer(m_Timer"));
        assert!(set_state.contains("ExSetTimer(m_Timer"));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn macos_driver_publishes_only_the_pulse_input() {
        let source = fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("audio-driver/macos/PulseAudio.swift"),
        )
        .expect("Pulse-owned macOS driver source must be bundled with the project");

        assert!(source.contains("private let kDeviceName = \"Pulse\""));
        assert!(source.contains("private let kChannels: UInt32 = 1"));
        assert!(source.contains("static let streamInput: AudioObjectID = 3"));
        assert!(!source.contains("streamOutput"));
        assert!(source.contains(
            "scope == kAudioObjectPropertyScopeOutput ? .empty : .objectID(Obj.streamInput)"
        ));
    }
}
