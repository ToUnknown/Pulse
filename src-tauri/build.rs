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
    tauri_build::build()
}
