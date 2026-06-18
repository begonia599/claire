fn main() {
    // 前端文件改动也要触发重新编译，否则 generate_context! 会嵌入旧的 HTML/JS
    println!("cargo:rerun-if-changed=../frontend");
    tauri_build::build()
}
