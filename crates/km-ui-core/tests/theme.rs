//! Интеграционные тесты CSS-токенов тем «Графит»/«Воздух».

use km_shared::Theme::{Dark, Light, System};
use km_ui_core::theme::{ACCENT, ACCENT_HOVER, ON_ACCENT, theme_vars};

/// Полный набор токенов, который обязан присутствовать в каждой теме.
const TOKENS: &[&str] = &[
    "--bg:",
    "--surface:",
    "--border:",
    "--text:",
    "--text-muted:",
    "--accent:",
    "--accent-hover:",
    "--on-accent:",
    "--accent-text:",
    "--control-border:",
    "--selected-bg:",
    "--input-bg:",
    "--matte:",
];

#[test]
fn accent_consts_pinned() {
    assert_eq!(ACCENT, "#e05616");
    assert_eq!(ACCENT_HOVER, "#ff7333");
    assert_eq!(ON_ACCENT, "#1a0d05");
}

#[test]
fn dark_graphite_vars() {
    let vars = theme_vars(Dark);
    assert!(vars.contains("--bg:#101113"), "dark bg: {vars}");
    assert!(vars.contains("--accent:#e05616"), "dark accent: {vars}");
    assert!(
        vars.contains("--accent-hover:#ff7333"),
        "dark accent-hover: {vars}"
    );
    for token in TOKENS {
        assert!(vars.contains(token), "нет токена {token} в dark: {vars}");
    }
}

#[test]
fn light_air_vars() {
    let vars = theme_vars(Light);
    assert!(vars.contains("--bg:#f8f8f7"), "light bg: {vars}");
    assert!(vars.contains("--accent:#e05616"), "light accent: {vars}");
    assert!(
        vars.contains("--accent-hover:#ff7333"),
        "light accent-hover: {vars}"
    );
    for token in TOKENS {
        assert!(vars.contains(token), "нет токена {token} в light: {vars}");
    }
}

#[test]
fn themes_differ() {
    assert_ne!(theme_vars(Dark), theme_vars(Light));
}

#[test]
fn system_defaults_to_light_until_ui_resolves() {
    // Задокументированный выбор: System резолвится в UI,
    // theme_vars(System) отдаёт светлый набор как дефолт.
    assert_eq!(theme_vars(System), theme_vars(Light));
}

#[test]
fn vars_match_accent_consts() {
    for theme in [Dark, Light] {
        let vars = theme_vars(theme);
        assert!(
            vars.contains(&format!("--accent:{ACCENT}")),
            "accent константа разошлась с токенами {theme:?}: {vars}"
        );
        assert!(
            vars.contains(&format!("--accent-hover:{ACCENT_HOVER}")),
            "accent-hover константа разошлась с токенами {theme:?}: {vars}"
        );
        assert!(
            vars.contains(&format!("--on-accent:{ON_ACCENT}")),
            "on-accent константа разошлась с токенами {theme:?}: {vars}"
        );
    }
}

#[test]
fn accent_text_meets_aa_on_both_surfaces() {
    fn luminance(vars: &str, key: &str) -> f64 {
        let (_, color) = vars.split_once(&format!("--{key}:#")).unwrap();
        [0.2126, 0.7152, 0.0722]
            .iter()
            .enumerate()
            .map(|(i, weight)| {
                let s =
                    f64::from(u8::from_str_radix(&color[2 * i..2 * i + 2], 16).unwrap()) / 255.0;
                weight
                    * if s <= 0.04045 {
                        s / 12.92
                    } else {
                        ((s + 0.055) / 1.055).powf(2.4)
                    }
            })
            .sum()
    }

    for theme in [Dark, Light] {
        let vars = theme_vars(theme);
        let text = luminance(vars, "accent-text");
        for surface in ["bg", "surface"] {
            let bg = luminance(vars, surface);
            assert!((text.max(bg) + 0.05) / (text.min(bg) + 0.05) >= 4.5);
        }
    }
}
