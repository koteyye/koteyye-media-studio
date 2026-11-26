// В release-сборке Windows скрывает консольное окно; не удалять.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Бинарь Koteyye Media Studio: делегирует запуск библиотечной точке входа.

fn main() {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--mcp-stdio")) {
        if let Err(err) = koteyye_media_studio::run_stdio() {
            eprintln!("{err}");
            std::process::exit(1);
        }
        return;
    }
    koteyye_media_studio::run()
}
