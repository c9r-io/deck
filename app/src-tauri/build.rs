fn main() {
    // The frontend is embedded into the binary at compile time; without these,
    // cargo doesn't know about it and UI-only edits silently ship stale.
    println!("cargo:rerun-if-changed=../ui/index.html");
    println!("cargo:rerun-if-changed=../ui/vendor");
    tauri_build::build()
}
