fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let source = "src/text_extractor/macos/native.m";
        println!("cargo:rerun-if-changed={source}");
        println!("cargo:rerun-if-changed=src/text_extractor/macos/native.h");
        cc::Build::new()
            .file(source)
            .flag("-fobjc-arc")
            .flag("-fblocks")
            .compile("pulse_capture");
        for framework in ["AppKit", "Vision", "CoreGraphics", "CoreText"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
        // Older Macs can still run the tray app; Text Extractor checks macOS 14
        // before touching ScreenCaptureKit instead of failing at process launch.
        println!("cargo:rustc-link-arg=-Wl,-weak_framework,ScreenCaptureKit");
    }
    tauri_build::build()
}
