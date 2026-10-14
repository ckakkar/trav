fn main() {
    // The app is macOS-only (see src/main.rs); skip Tauri's codegen elsewhere.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        tauri_build::build()
    }
}
