// Embeds the ghost icon in the Windows executable (Explorer, taskbar, shortcuts).
// macOS and Linux get their icons from the app bundle / .desktop file instead.
fn main() {
    println!("cargo:rerun-if-changed=assets/icons/ghostlane.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icons/ghostlane.ico");
        res.compile().expect("failed to embed the Windows icon");
    }
}
