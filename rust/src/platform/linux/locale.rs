//! Linux: interface language and local time of the system.

use crate::i18n::Idioma;

pub use crate::platform::unix::locale::agora;

/// The locale of the session (LC_ALL, LC_MESSAGES or LANG).
pub fn idioma() -> Idioma {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_default();
    crate::platform::unix::locale::pelo_locale(&locale)
}
