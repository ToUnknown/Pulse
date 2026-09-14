use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rustc-check-cfg=cfg(pulse_apple_pcc)");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        build_apple_intelligence();
        let source = "src/text_extractor/macos/native.m";
        println!("cargo:rerun-if-changed={source}");
        println!("cargo:rerun-if-changed=src/text_extractor/macos/native.h");
        cc::Build::new()
            .file(source)
            .flag("-fobjc-arc")
            .flag("-fblocks")
            .compile("pulse_capture");
        for framework in ["AppKit", "Vision", "CoreGraphics"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
        // Older Macs can still run the tray app; Text Extractor checks macOS 14
        // before touching ScreenCaptureKit instead of failing at process launch.
        println!("cargo:rustc-link-arg=-Wl,-weak_framework,ScreenCaptureKit");
    }
    tauri_build::build()
}

fn xcrun(arguments: &[&str]) -> String {
    let output = Command::new("xcrun")
        .args(arguments)
        .output()
        .expect("Xcode command-line tools are required");
    assert!(
        output.status.success(),
        "xcrun failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn build_apple_intelligence() {
    println!("cargo:rerun-if-env-changed=DEVELOPER_DIR");
    println!("cargo:rerun-if-env-changed=SDKROOT");
    println!("cargo:rerun-if-changed=src/text_extractor/macos/AppleIntelligence.swift");
    let version = xcrun(&["--sdk", "macosx", "--show-sdk-version"]);
    if version
        .split('.')
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0)
        < 27
    {
        println!("cargo:warning=Apple cloud models require Xcode 27; this build will show an unavailable status with OpenAI and Basic still usable.");
        return;
    }
    let sdk = xcrun(&["--sdk", "macosx", "--show-sdk-path"]);
    let swift = PathBuf::from(xcrun(&["--find", "swiftc"]));
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let object = out.join("AppleIntelligence.o");
    let arch = if env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64") {
        "arm64"
    } else {
        "x86_64"
    };
    // Match the app minimum: the Swift concurrency runtime is part of macOS 12+.
    let target = format!("{arch}-apple-macos12.0");
    let result = Command::new(&swift)
        .args([
            "-parse-as-library",
            "-emit-object",
            "-O",
            "-swift-version",
            "6",
            "-module-name",
            "PulseAppleIntelligence",
            "-sdk",
            &sdk,
            "-target",
            &target,
            "-Xfrontend",
            "-disable-autolink-framework",
            "-Xfrontend",
            "FoundationModels",
            "-module-cache-path",
        ])
        .arg(out.join("swift-module-cache"))
        .arg("src/text_extractor/macos/AppleIntelligence.swift")
        .arg("-o")
        .arg(&object)
        .output()
        .expect("Could not compile the Apple Intelligence bridge");
    assert!(
        result.status.success(),
        "Apple Intelligence bridge failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    println!("cargo:rustc-cfg=pulse_apple_pcc");
    cc::Build::new()
        .object(object)
        .compile("pulse_apple_intelligence");
    let libraries = swift
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("lib/swift/macosx");
    println!("cargo:rustc-link-search=native={}", libraries.display());
    println!("cargo:rustc-link-search=native=/usr/lib/swift");
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    println!("cargo:rustc-link-arg=-Wl,-weak_framework,FoundationModels");
    println!("cargo:rustc-link-lib=framework=Security");
}
