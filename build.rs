fn main() {
    println!("cargo:rerun-if-changed=linker.ld");
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-search={dir}");
    println!("cargo:rustc-link-arg=-Tlinker.ld");
}
