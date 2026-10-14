#[cfg(not(target_os = "macos"))]
compile_error!(
    "The Trav desktop app is macOS-only. Elsewhere, use the `trav` command: \
     the terminal UI, `trav get`, or `trav --daemon` for the web UI."
);

fn main() {
    #[cfg(target_os = "macos")]
    trav_desktop_lib::run()
}
