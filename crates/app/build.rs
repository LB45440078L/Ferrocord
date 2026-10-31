//! Build script for the `ferrocord` binary.
//!
//! On Windows targets it embeds the application icon and a `VERSIONINFO`
//! resource (file/product version from the workspace version) into
//! `ferrocord.exe`. Other targets are untouched. The decision is made from the
//! *target* (`CARGO_CFG_TARGET_OS`), not the host, so cross-compiling to or
//! from Windows works: MSVC hosts use `rc.exe` from the Windows SDK, other
//! hosts use `llvm-rc` (MSVC targets) or `windres` (GNU targets).
//!
//! A missing resource compiler is a warning for local builds. Set
//! `FERROCORD_REQUIRE_WINRES=1` (release CI does) to make it a hard error.

use std::env;

const ICON: &str = "../../packaging/icons/ferrocord.ico";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={ICON}");
    println!("cargo:rerun-if-env-changed=FERROCORD_REQUIRE_WINRES");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_windows_resources();
    }
}

fn embed_windows_resources() {
    let version = env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let icon = std::path::Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap_or_default()).join(ICON);
    let mut res = winresource::WindowsResource::new();
    res.set_icon(&icon.to_string_lossy())
        .set_language(0x0409) // en-US
        .set("ProductName", "Ferrocord")
        .set("FileDescription", "Ferrocord")
        .set("CompanyName", "AmberCode")
        .set("LegalCopyright", "Copyright (c) 2026 AmberCode. BSD-3-Clause license.")
        .set("OriginalFilename", "ferrocord.exe")
        .set("InternalName", "ferrocord")
        .set("FileVersion", &version)
        .set("ProductVersion", &version);

    if let Err(err) = res.compile_for("ferrocord") {
        if env::var_os("FERROCORD_REQUIRE_WINRES").is_some_and(|v| !v.is_empty() && v != "0") {
            panic!("failed to embed Windows resources (icon, version info): {err}");
        }
        println!("cargo:warning=ferrocord.exe will have no icon/version info: resource compiler failed: {err}");
    }
}
