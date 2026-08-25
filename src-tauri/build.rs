#[cfg(target_os = "macos")]
fn build_macos_audio_driver() {
    use std::{env, fs, path::Path, process::Command, time::SystemTime};

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    let manifest = env::current_dir().expect("Cargo manifest directory");
    let source = manifest.join("audio-driver/macos/PulseAudio.swift");
    let plist = manifest.join("audio-driver/macos/Info.plist");
    let output_root = manifest.join("target/pulse-audio-driver");
    let bundle = output_root.join("Pulse.driver");
    let executable = bundle.join("Contents/MacOS/PulseAudio");

    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed={}", plist.display());

    let modified = |path: &Path| {
        fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH)
    };
    if executable.is_file()
        && modified(&executable) >= modified(&source)
        && modified(&executable) >= modified(&plist)
    {
        return;
    }

    let build = output_root.join("build");
    let _ = fs::remove_dir_all(&bundle);
    let _ = fs::remove_dir_all(&build);
    fs::create_dir_all(bundle.join("Contents/MacOS")).expect("Pulse driver bundle directory");
    fs::create_dir_all(&build).expect("Pulse driver build directory");
    fs::copy(&plist, bundle.join("Contents/Info.plist")).expect("Pulse driver Info.plist");

    let module_cache = output_root.join("module-cache");
    fs::create_dir_all(&module_cache).expect("Swift module cache");
    let mut slices = Vec::new();
    for architecture in ["arm64", "x86_64"] {
        let slice = build.join(format!("PulseAudio-{architecture}"));
        let status = Command::new("swiftc")
            .args([
                "-O",
                "-parse-as-library",
                "-emit-library",
                "-Xlinker",
                "-bundle",
                "-framework",
                "CoreAudio",
                "-framework",
                "CoreFoundation",
                "-target",
                &format!("{architecture}-apple-macos11.0"),
                "-module-name",
                "PulseAudio",
                "-o",
            ])
            .arg(&slice)
            .arg(&source)
            .env("CLANG_MODULE_CACHE_PATH", &module_cache)
            .status()
            .expect("run swiftc for the Pulse audio driver");
        assert!(status.success(), "could not compile the Pulse audio driver");
        slices.push(slice);
    }

    let status = Command::new("lipo")
        .arg("-create")
        .args(&slices)
        .arg("-output")
        .arg(&executable)
        .status()
        .expect("run lipo for the Pulse audio driver");
    assert!(
        status.success(),
        "could not create the universal Pulse audio driver"
    );

    let status = Command::new("codesign")
        .args(["--force", "--sign", "-"])
        .arg(&bundle)
        .status()
        .expect("ad-hoc sign the Pulse audio driver");
    assert!(status.success(), "could not sign the Pulse audio driver");
    fs::remove_dir_all(&build).expect("remove Pulse driver intermediate files");
}

#[cfg(target_os = "windows")]
fn prepare_windows_audio_driver_resource() {
    use std::{env, fs, path::PathBuf};

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"));
    let package = manifest.join("target/audio-driver/windows/x64/Release/package");
    fs::create_dir_all(&package).expect("Pulse Windows driver package directory");

    if env::var("PROFILE").as_deref() == Ok("release") {
        for required in [
            "PulseVirtualMic.inf",
            "PulseVirtualMic.sys",
            "PulseVirtualMic.cat",
            "PulseDriverInstaller.exe",
        ] {
            let path = package.join(required);
            assert!(
                path.is_file(),
                "the release driver package is incomplete: {} is missing; run scripts/build-windows-driver.ps1 first",
                path.display()
            );
        }
    }
}

fn main() {
    #[cfg(target_os = "macos")]
    build_macos_audio_driver();
    #[cfg(target_os = "windows")]
    prepare_windows_audio_driver_resource();
    tauri_build::build()
}
