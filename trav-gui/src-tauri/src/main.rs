#[cfg(not(target_os = "macos"))]
compile_error!("Trav is a macOS app.");

fn main() {
    #[cfg(target_os = "macos")]
    trav_desktop_lib::run()
}
