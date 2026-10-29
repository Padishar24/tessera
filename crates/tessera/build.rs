fn main() {
    for file in [
        "tessera.rc",
        "tesserac.rc",
        "common.rc2",
        "tessera.exe.manifest",
        "tessera.ico",
    ] {
        println!("cargo:rerun-if-changed={file}");
    }

    let version = |key: &str| std::env::var(key).unwrap();
    let macros = [
        format!("VER_MAJOR={}", version("CARGO_PKG_VERSION_MAJOR")),
        format!("VER_MINOR={}", version("CARGO_PKG_VERSION_MINOR")),
        format!("VER_PATCH={}", version("CARGO_PKG_VERSION_PATCH")),
        format!("VER_STRING=\"{}\"", version("CARGO_PKG_VERSION")),
    ];

    // Embed manifest (per-monitor DPI v2, Common Controls 6), icon and version
    // info in both programs.
    for bin in ["tessera", "tesserac"] {
        embed_resource::compile_for(format!("{bin}.rc"), [bin], &macros)
            .manifest_required()
            .unwrap();
    }
}
