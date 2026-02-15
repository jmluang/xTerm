fn main() {
    // Ensure changes to window chrome settings take effect without manual cargo clean.
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=capabilities/default.json");
    println!("cargo:rerun-if-changed=capabilities");
    tauri_build::build()
}
