//! Windows: the icon and version information of retrogit.exe.

fn main() {
    println!("cargo:rerun-if-env-changed=RETROGIT_VERSION");
    println!("cargo:rerun-if-changed=assets/RetroGit.ico");
    #[cfg(windows)]
    windows_resources();
}

#[cfg(windows)]
fn windows_resources() {
    let version = std::env::var("RETROGIT_VERSION")
        .unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap_or_default());
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/RetroGit.ico")
        .set("ProductName", "RetroGit")
        .set("FileDescription", "RetroGit")
        .set("ProductVersion", &version)
        .set("FileVersion", &version)
        .set("OriginalFilename", "retrogit.exe");
    if let Err(e) = res.compile() {
        // A missing resource compiler must not break the build: the exe has no icon then.
        println!("cargo:warning=no Windows resources: {e}");
    }
}
