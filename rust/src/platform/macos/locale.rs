//! macOS: interface language and local time of the system.

use crate::i18n::Idioma;
use crate::platform::unix::locale::pelo_locale;

pub use crate::platform::unix::locale::agora;

/// The first language of System Settings (apps opened from Finder have no
/// LANG); LANG when it cannot be read.
pub fn idioma() -> Idioma {
    let lista = std::process::Command::new("/usr/bin/defaults")
        .args(["read", "-g", "AppleLanguages"])
        .output()
        .ok()
        .and_then(|s| primeira_lingua(&String::from_utf8_lossy(&s.stdout)));
    match lista {
        Some(l) => pelo_locale(&l),
        None => pelo_locale(&std::env::var("LANG").unwrap_or_default()),
    }
}

/// First item of `defaults read -g AppleLanguages`:
/// `(\n    "pt-BR",\n    "en-US"\n)` -> "pt-BR".
fn primeira_lingua(texto: &str) -> Option<String> {
    texto
        .lines()
        .map(|l| l.trim().trim_end_matches(',').trim_matches('"'))
        .find(|l| !l.is_empty() && *l != "(" && *l != ")")
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idioma_do_macos() {
        let saida = "(\n    \"pt-BR\",\n    \"en-US\"\n)\n";
        assert_eq!(primeira_lingua(saida).as_deref(), Some("pt-BR"));
        assert_eq!(primeira_lingua("(\n    en,\n)\n").as_deref(), Some("en"));
        assert_eq!(primeira_lingua(""), None);
    }
}
