fn main() {
    // The pages are embedded when the program is built, so changing one means rebuilding.
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=capabilities");
    tauri_build::build();
}
