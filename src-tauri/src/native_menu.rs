//! The macOS menu uses the same resolved language as the studio.

use km_shared::{Lang, LangPreference};
use tauri::AppHandle;
use tauri::menu::{
    AboutMetadata, HELP_SUBMENU_ID, Menu, PredefinedMenuItem as Item, Submenu, WINDOW_SUBMENU_ID,
};

pub(crate) fn apply(app: &AppHandle, preference: LangPreference) {
    let lang = preference.resolve(Lang::detect_system().unwrap_or(Lang::En));
    if let Err(err) = build(app, lang).and_then(|menu| app.set_menu(menu).map(|_| ())) {
        eprintln!("Could not apply the studio menu language: {err}");
    }
}

fn build(app: &AppHandle, lang: Lang) -> tauri::Result<Menu<tauri::Wry>> {
    let label = |ru: &'static str, en: &'static str| match lang {
        Lang::Ru => ru,
        Lang::En => en,
    };
    let text = |ru, en| Some(label(ru, en));
    let package = app.package_info();
    let about = AboutMetadata {
        name: Some(package.name.clone()),
        version: Some(package.version.to_string()),
        copyright: app.config().bundle.copyright.clone(),
        authors: app.config().bundle.publisher.clone().map(|name| vec![name]),
        ..Default::default()
    };
    let about_title = format!("{} {}", label("О приложении", "About"), package.name);
    let hide_title = format!("{} {}", label("Скрыть", "Hide"), package.name);
    let quit_title = format!("{} {}", label("Завершить", "Quit"), package.name);
    let app_menu = Submenu::with_items(
        app,
        &package.name,
        true,
        &[
            &Item::about(app, Some(&about_title), Some(about))?,
            &Item::separator(app)?,
            &Item::services(app, text("Службы", "Services"))?,
            &Item::separator(app)?,
            &Item::hide(app, Some(&hide_title))?,
            &Item::hide_others(app, text("Скрыть остальные", "Hide Others"))?,
            &Item::separator(app)?,
            &Item::quit(app, Some(&quit_title))?,
        ],
    )?;
    let close_window = text("Закрыть окно", "Close Window");
    let file = Submenu::with_items(
        app,
        label("Файл", "File"),
        true,
        &[&Item::close_window(app, close_window)?],
    )?;
    let edit = Submenu::with_items(
        app,
        label("Правка", "Edit"),
        true,
        &[
            &Item::undo(app, text("Отменить", "Undo"))?,
            &Item::redo(app, text("Повторить", "Redo"))?,
            &Item::separator(app)?,
            &Item::cut(app, text("Вырезать", "Cut"))?,
            &Item::copy(app, text("Скопировать", "Copy"))?,
            &Item::paste(app, text("Вставить", "Paste"))?,
            &Item::select_all(app, text("Выбрать всё", "Select All"))?,
        ],
    )?;
    let view = Submenu::with_items(
        app,
        label("Вид", "View"),
        true,
        &[&Item::fullscreen(
            app,
            text("Полноэкранный режим", "Full Screen"),
        )?],
    )?;
    let window = Submenu::with_id_and_items(
        app,
        WINDOW_SUBMENU_ID,
        label("Окно", "Window"),
        true,
        &[
            &Item::minimize(app, text("Свернуть", "Minimize"))?,
            &Item::maximize(app, text("Увеличить", "Zoom"))?,
            &Item::separator(app)?,
            &Item::close_window(app, close_window)?,
        ],
    )?;
    let help =
        Submenu::with_id_and_items(app, HELP_SUBMENU_ID, label("Справка", "Help"), true, &[])?;
    Menu::with_items(app, &[&app_menu, &file, &edit, &view, &window, &help])
}
