fn main() {
    let output =
        std::process::Command::new(std::env::var_os("RUSTC").expect("Cargo supplies RUSTC"))
            .arg("-vV")
            .output()
            .expect("read compiler version");
    assert!(output.status.success());
    let version = String::from_utf8(output.stdout).expect("UTF-8 compiler version");
    println!(
        "cargo:rustc-env=VERIFIED_RUSTC={}",
        version.replace('\n', " | ")
    );
    println!("cargo:rerun-if-changed=build.rs");
}
