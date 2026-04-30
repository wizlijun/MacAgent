fn main() {
    // 让 cargo 在 assets/tray-icon.png 变化时重跑构建
    println!("cargo:rerun-if-changed=assets/tray-icon.png");
}
