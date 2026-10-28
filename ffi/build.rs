//! Writes `bosphorus_ffi.h` from `src/lib.rs` (cbindgen), like lez-faucet-ffi.

fn main() {
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");
    // Found through the loader's rpath (a module's bundle), not at the path it was built at.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,-install_name,@rpath/libbosphorus_ffi.dylib");
    }

    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set");
    let config = cbindgen::Config::from_file(format!("{crate_dir}/cbindgen.toml")).expect("valid cbindgen config");
    cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(config)
        .generate()
        .expect("generate the Bosphorus FFI header")
        .write_to_file(format!("{crate_dir}/bosphorus_ffi.h"));
}
