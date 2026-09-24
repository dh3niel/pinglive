// Embeds the icon and the Windows version info (Details tab in the file's
// Properties, the publisher shown by Windows) into pinglive.exe.
fn main() {
    println!("cargo:rerun-if-changed=assets/pinglive.ico");
    println!("cargo:rerun-if-changed=Cargo.toml"); // authors -> publisher
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let author = std::env::var("CARGO_PKG_AUTHORS").unwrap_or_default();
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/pinglive.ico")
        .set("ProductName", "PingLive")
        .set("FileDescription", "PingLive ping overlay")
        .set("CompanyName", &author)
        .set("LegalCopyright", &format!("Copyright (c) 2026 {author}. MIT License"))
        .set("OriginalFilename", "pinglive.exe")
        .set("InternalName", "pinglive")
        .set("Comments", env!("CARGO_PKG_DESCRIPTION"));
    res.compile().expect("embedding the Windows icon/version resource failed");
}
