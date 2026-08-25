use std::{fs, io, path::PathBuf, process::Command};

#[cfg(target_os = "windows")]
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use cpal::traits::{DeviceTrait, HostTrait};
use serde::Serialize;
#[cfg(target_os = "windows")]
use std::path::Path;
use tauri::{path::BaseDirectory, AppHandle, Manager};
#[cfg(target_os = "windows")]
use tempfile::TempDir;

const OWNER_MARKER: &str = "pulse-installed-vb-cable";
#[cfg(target_os = "windows")]
const WINDOWS_PACKAGE: &str = "resources/virtual-audio/VBCABLE_Driver_Pack45.zip";
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
    let provider_was_present = backing_provider_present();
    if !provider_was_present {
        let package = bundled_windows_package(app)?;
        let unpacked = unpack_windows_package(&package)?;
        install_windows_provider(&unpacked)?;
        mark_provider_owned(app, OwnedProvider::LegacyVbCable)?;
    } else {
        rename_windows_capture_endpoint("Pulse")?;
    }

    Ok(LifecycleResult {
        restart_required: !wait_for_pulse_state(true),
    })
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
    let owned = owned_provider(app)?.is_some();
    if owned {
        let package = bundled_windows_package(app)?;
        let unpacked = unpack_windows_package(&package)?;
        remove_windows_provider(&unpacked)?;
        if !backing_provider_present() {
            clear_provider_owned(app)?;
        }
    } else {
        rename_windows_capture_endpoint("CABLE Output")?;
    }

    Ok(LifecycleResult {
        restart_required: !wait_for_pulse_state(false) || (owned && backing_provider_present()),
    })
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
    let path = app
        .path()
        .resolve(WINDOWS_PACKAGE, BaseDirectory::Resource)
        .map_err(|error| format!("could not locate the bundled Pulse audio component: {error}"))?;
    path.is_file()
        .then_some(path)
        .ok_or_else(|| "the bundled Pulse audio component is missing".to_string())
}

#[cfg(target_os = "windows")]
fn unpack_windows_package(package: &Path) -> Result<TempDir, String> {
    let directory = tempfile::Builder::new()
        .prefix("pulse-virtual-audio-")
        .tempdir()
        .map_err(|error| format!("could not prepare the Pulse audio installer: {error}"))?;

    let status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Expand-Archive -LiteralPath $env:PULSE_AUDIO_ARCHIVE -DestinationPath $env:PULSE_AUDIO_DESTINATION -Force",
        ])
        .env("PULSE_AUDIO_ARCHIVE", package)
        .env("PULSE_AUDIO_DESTINATION", directory.path())
        .status();

    let status =
        status.map_err(|error| format!("could not unpack the Pulse audio component: {error}"))?;
    if !status.success() {
        return Err("could not unpack the Pulse audio component".to_string());
    }

    Ok(directory)
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
        Ok(value) if value.trim() == "PULSE_DRIVER" => Ok(Some(OwnedProvider::PulseDriver)),
        Ok(_) => Ok(Some(OwnedProvider::LegacyVbCable)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
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
fn install_windows_provider(unpacked: &TempDir) -> Result<(), String> {
    run_windows_setup(unpacked, true)?;
    if !backing_provider_present() && !windows_provider_service_exists() {
        return Err("Pulse audio installation was cancelled or did not finish".to_string());
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn remove_windows_provider(unpacked: &TempDir) -> Result<(), String> {
    run_windows_setup(unpacked, false)
}

#[cfg(target_os = "windows")]
fn windows_provider_service_exists() -> bool {
    Command::new("sc.exe")
        .args(["query", "VBAudioVACMME"])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(target_os = "windows")]
fn run_windows_setup(unpacked: &TempDir, rename_after: bool) -> Result<(), String> {
    let setup = unpacked.path().join("VBCABLE_Setup_x64.exe");
    let mut elevated = String::from(
        "$p = Start-Process -FilePath $env:PULSE_VIRTUAL_AUDIO_SETUP -WorkingDirectory $env:PULSE_VIRTUAL_AUDIO_DIRECTORY -Wait -PassThru; \
         if ($p.ExitCode -ne 0) { exit $p.ExitCode }; ",
    );
    if rename_after {
        elevated.push_str(&windows_rename_script("Pulse"));
    }
    let encoded = encode_powershell(&elevated);
    run_elevated_powershell(&encoded, Some(&setup))
}

#[cfg(target_os = "windows")]
fn rename_windows_capture_endpoint(name: &str) -> Result<(), String> {
    let encoded = encode_powershell(&windows_rename_script(name));
    run_elevated_powershell(&encoded, None)
}

#[cfg(target_os = "windows")]
fn windows_rename_script(name: &str) -> String {
    format!(
        r#"
$root = 'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture'
$property = '{{a45c254e-df1c-4efd-8020-67d146a850e0}},14'
if (Test-Path -LiteralPath $root) {{
  Get-ChildItem -LiteralPath $root | ForEach-Object {{
    $properties = Join-Path $_.PSPath 'Properties'
    if (Test-Path -LiteralPath $properties) {{
      $current = (Get-ItemProperty -LiteralPath $properties -Name $property -ErrorAction SilentlyContinue).$property
      if ($current -eq 'Pulse' -or $current -like '*CABLE Output*') {{
        Set-ItemProperty -LiteralPath $properties -Name $property -Value '{name}'
      }}
    }}
  }}
}}
"#
    )
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
fn run_elevated_powershell(encoded: &str, setup: Option<&Path>) -> Result<(), String> {
    const LAUNCH: &str = "$p = Start-Process -FilePath 'powershell.exe' -ArgumentList @('-NoProfile','-NonInteractive','-EncodedCommand',$env:PULSE_ELEVATED_COMMAND) -Verb RunAs -Wait -PassThru; exit $p.ExitCode";
    let mut command = Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-NonInteractive", "-Command", LAUNCH])
        .env("PULSE_ELEVATED_COMMAND", encoded);
    if let Some(setup) = setup {
        command.env("PULSE_VIRTUAL_AUDIO_SETUP", setup);
        if let Some(directory) = setup.parent() {
            command.env("PULSE_VIRTUAL_AUDIO_DIRECTORY", directory);
        }
    }
    let status = command
        .status()
        .map_err(|error| format!("could not start the Pulse audio installer: {error}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| "Pulse audio installation or removal was cancelled".to_string())
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
