//! Embeds `monaka.manifest` into the window's executable (not `monaka_play`): without it Windows draws
//! the controls in the old style and blurs the window on high-DPI screens.

fn main() {
    let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("monaka.manifest");
    println!("cargo:rerun-if-changed=monaka.manifest");
    // The path above is absolute: after the folder moves, run again for the new one.
    println!("cargo:rerun-if-env-changed=CARGO_MANIFEST_DIR");
    println!("cargo:rustc-link-arg-bin=monaka=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bin=monaka=/MANIFESTINPUT:{}", manifest.display());
}
