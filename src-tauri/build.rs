//! Сборочный скрипт Tauri: генерирует контекст приложения из tauri.conf.json.

fn main() {
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
