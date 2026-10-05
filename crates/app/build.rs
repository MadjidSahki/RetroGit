//! Windows targets: the icon and version information of retrogit.exe. Decided by the
//! target (`CARGO_CFG_TARGET_OS`), not by the machine building, so cross builds get them too.

#[allow(dead_code)]
#[path = "src/version.rs"]
mod version;

fn main() {
    println!("cargo:rerun-if-env-changed=RETROGIT_VERSION");
    println!("cargo:rerun-if-changed=assets/RetroGit.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_resources();
    }
}

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
    // The numeric fields (Explorer's "File version") default to Cargo's version.
    if let Some(n) = version::numeric_version(&version) {
        res.set_version_info(winresource::VersionInfo::FILEVERSION, n)
            .set_version_info(winresource::VersionInfo::PRODUCTVERSION, n);
    }
    if let Err(e) = res.compile() {
        // A missing resource compiler must not break the build: the exe has no icon then.
        println!("cargo:warning=no Windows resources: {e}");
    }
}
