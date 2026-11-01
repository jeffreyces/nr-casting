//! Give the DLL the file-identity every scanner and every support report looks at.
//!
//! A DLL with no resource section is, to Windows and to every antivirus UI, an anonymous payload:
//! no product name, no publisher, no version, nothing to show the player when it is quarantined and
//! nothing for a reputation service to attach a verdict to. Embedded resources are cheap, they are
//! what a legitimate signed application has, and their absence is one of the signals a static
//! classifier weighs.
//!
//! The version block is generated from `CARGO_PKG_VERSION` rather than kept as a hand-written `.rc`
//! so the DLL, the zip name and the release notes cannot disagree.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // Re-run only when the identity inputs change: a version bump, or this file.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");

    let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    // A four-field version, which is what VERSIONINFO expects: 0.2.9 -> 0.2.9.0.
    let comma = {
        let mut parts: Vec<String> = version.split('.').map(|p| p.parse().unwrap_or(0).to_string()).collect();
        while parts.len() < 4 {
            parts.push("0".into());
        }
        parts.join(",")
    };

    // Anything a user or a support volunteer might need to read off the file's Properties dialog.
    // `CompanyName` is a person here, deliberately: pretending to be a company is worse than being
    // an individual author, and an unverifiable company name is itself a scoring signal.
    let rc = format!(
        r#"#include <winver.h>

VS_VERSION_INFO VERSIONINFO
 FILEVERSION {comma}
 PRODUCTVERSION {comma}
 FILEFLAGSMASK 0x3fL
 FILEFLAGS 0x0L
 FILEOS VOS_NT_WINDOWS32
 FILETYPE VFT_DLL
 FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName",      "jeffreyces"
            VALUE "FileDescription",  "nr-casting - four spells per catalyst (Elden Ring mod)"
            VALUE "FileVersion",      "{version}"
            VALUE "InternalName",     "nr_casting"
            VALUE "LegalCopyright",   "MIT Licence"
            VALUE "OriginalFilename", "nr_casting.dll"
            VALUE "ProductName",      "nr-casting"
            VALUE "ProductVersion",   "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#
    );

    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let rc_path = out.join("nr_casting.rc");
    match fs::write(&rc_path, rc) {
        Ok(()) => {}
        Err(e) => {
            // A missing version block must never cost somebody a build: the mod still works, it
            // just looks anonymous again. Say so and carry on.
            println!("cargo:warning=could not write {rc_path:?}: {e}; DLL will have no version resource");
            return;
        }
    }

    // The resource is only meaningful for Windows targets; a host `cargo test` of the pure crate
    // must not try to run windres.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if let Err(e) = embed_resource::compile(&rc_path, embed_resource::NONE).manifest_optional() {
        println!("cargo:warning=version resource not embedded: {e}");
    }
    println!("cargo:rerun-if-changed={}", rc_path.display());
}
