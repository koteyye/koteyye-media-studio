// В release-сборке Windows скрывает консольное окно; не удалять.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Бинарь Koteyye Media Studio: делегирует запуск библиотечной точке входа.

fn main() {
    koteyye_media_studio::run()
}
