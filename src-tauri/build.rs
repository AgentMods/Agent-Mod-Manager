fn main() {
    // Tauri embeds its Common Controls v6 manifest only into binaries. The
    // library unit-test harness imports TaskDialogIndirect through rfd, so it
    // needs the same manifest or Windows rejects it with
    // STATUS_ENTRYPOINT_NOT_FOUND. Tauri documents this linker-based setup.
    let windows_msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let mut windows = tauri_build::WindowsAttributes::new();
    if windows_msvc {
        windows = tauri_build::WindowsAttributes::new_without_app_manifest();
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("failed to run Tauri build script");
}
