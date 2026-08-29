use std::{fs, io, path::PathBuf, process::Command};

use cpal::traits::{DeviceTrait, HostTrait};
use serde::Serialize;
use tauri::{path::BaseDirectory, AppHandle, Manager};

const OWNER_MARKER: &str = "pulse-installed-vb-cable";
const MACOS_DRIVER: &str = "target/pulse-audio-driver/Pulse.driver";
const INSTALLED_MACOS_DRIVER: &str = "/Library/Audio/Plug-Ins/HAL/Pulse.driver";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LifecycleResult {
    pub(crate) restart_required: bool,
}

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

fn bundled_macos_driver(app: &AppHandle) -> Result<PathBuf, String> {
    let path = app
        .path()
        .resolve(MACOS_DRIVER, BaseDirectory::Resource)
        .map_err(|error| format!("could not locate the bundled Pulse audio driver: {error}"))?;
    path.is_dir()
        .then_some(path)
        .ok_or_else(|| "the bundled Pulse audio driver is missing".to_string())
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
}

#[cfg(test)]
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
