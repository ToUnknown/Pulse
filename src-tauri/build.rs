fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let source = "src/text_extractor/macos/native.m";
        println!("cargo:rerun-if-changed={source}");
        println!("cargo:rerun-if-changed=src/text_extractor/macos/native.h");
        cc::Build::new()
            .file(source)
            .file("src/dictation/native.m")
            .flag("-fobjc-arc")
            .flag("-fblocks")
            .compile("pulse_capture");
        println!("cargo:rerun-if-changed=src/dictation/native.m");
        for framework in [
            "AppKit",
            "QuartzCore",
            "Vision",
            "CoreGraphics",
            "CoreText",
            "AVFoundation",
            "ApplicationServices",
        ] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
        // Older Macs can still run the tray app; Text Extractor checks macOS 14
        // before touching ScreenCaptureKit instead of failing at process launch.
        println!("cargo:rustc-link-arg=-Wl,-weak_framework,ScreenCaptureKit");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        cc::Build::new()
            .cpp(true)
            .file("src/dictation/windows.cpp")
            .flag_if_supported("/std:c++17")
            .flag_if_supported("/utf-8")
            .compile("pulse_dictation_windows");
        println!("cargo:rerun-if-changed=src/dictation/windows.cpp");
        for library in [
            "ole32",
            "oleaut32",
            "uiautomationcore",
            "user32",
            "shell32",
            "advapi32",
        ] {
            println!("cargo:rustc-link-lib={library}");
        }
    }
    tauri_build::build()
}
