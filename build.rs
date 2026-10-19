// Embeds the ghost icon in the Windows executable (Explorer, taskbar, shortcuts), and the greyscale ghost second,
// as icon index 1, for .ghost files (the installer points them at it).
// macOS and Linux get their icons from the app bundle / .desktop file instead.
fn main() {
    println!("cargo:rerun-if-changed=assets/icons/ghostlane.ico");
    println!("cargo:rerun-if-changed=assets/icons/ghostlane-file.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icons/ghostlane.ico");
        res.set_icon_with_id("assets/icons/ghostlane-file.ico", "2");
        res.compile().expect("failed to embed the Windows icon");
    }
}
