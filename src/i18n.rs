use std::sync::atomic::{AtomicU8, Ordering};

use crate::navigation_state::Language;
use gpui::SharedString;

mod context_menu;
mod italian;
mod library;
mod lyrics;
mod playback;
mod search;
mod shell;

static CURRENT_LANGUAGE: AtomicU8 = AtomicU8::new(0);

pub(crate) fn set_language(language: Language) {
    CURRENT_LANGUAGE.store(language_code(language), Ordering::Release);
}

pub(crate) fn language() -> Language {
    match CURRENT_LANGUAGE.load(Ordering::Acquire) {
        1 => Language::Italian,
        _ => Language::English,
    }
}

pub(crate) fn text(source: &'static str) -> &'static str {
    match language() {
        Language::English => source,
        Language::Italian => translate(source).unwrap_or(source),
    }
}

pub(crate) fn shared_text(source: impl AsRef<str>) -> SharedString {
    let source = source.as_ref();
    let translated = match language() {
        Language::English => source,
        Language::Italian => translate(source).unwrap_or(source),
    };
    SharedString::from(translated.to_owned())
}

pub(crate) fn format_template(source: &'static str, values: &[&str]) -> String {
    let mut formatted = text(source).to_owned();
    for value in values {
        formatted = formatted.replacen("{}", value, 1);
    }
    formatted
}

fn translate(source: &str) -> Option<&'static str> {
    italian::translate(source)
        .or_else(|| context_menu::translate(source))
        .or_else(|| library::translate(source))
        .or_else(|| lyrics::translate(source))
        .or_else(|| playback::translate(source))
        .or_else(|| search::translate(source))
        .or_else(|| shell::translate(source))
}

const fn language_code(language: Language) -> u8 {
    match language {
        Language::English => 0,
        Language::Italian => 1,
    }
}
