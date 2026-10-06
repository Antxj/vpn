//! Interface language: Portuguese (Brazil) or English.
//!
//! Default: the Windows language (Portuguese -> pt-BR; anything else ->
//! English). The user can pin either one on the accounts screen.
//!
//! The texts live in the code itself, side by side:
//! `tr!("Conectar", "Connect")` returns &'static str and
//! `trf!("{n} de {total}", "{n} of {total}")` builds a String (format!).

use std::sync::atomic::{AtomicBool, Ordering};

/// Fixed text in the current language.
macro_rules! tr {
    ($pt:literal, $en:literal $(,)?) => {
        if $crate::i18n::pt() {
            $pt
        } else {
            $en
        }
    };
}

/// Formatted text (like format!) in the current language.
macro_rules! trf {
    ($pt:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        if $crate::i18n::pt() {
            format!($pt $(, $arg)*)
        } else {
            format!($en $(, $arg)*)
        }
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Idioma {
    Portugues,
    Ingles,
}

impl Idioma {
    /// Code stored in the settings.
    pub fn codigo(self) -> &'static str {
        match self {
            Idioma::Portugues => "pt",
            Idioma::Ingles => "en",
        }
    }

    pub fn do_codigo(c: &str) -> Option<Idioma> {
        match c {
            "pt" => Some(Idioma::Portugues),
            "en" => Some(Idioma::Ingles),
            _ => None,
        }
    }

    /// Language name written in itself (for the selector).
    pub fn nome(self) -> &'static str {
        match self {
            Idioma::Portugues => "Português",
            Idioma::Ingles => "English",
        }
    }
}

// When not initialized (tests), it stays in Portuguese.
static PORTUGUES: AtomicBool = AtomicBool::new(true);

pub fn pt() -> bool {
    PORTUGUES.load(Ordering::Relaxed)
}

/// Applies the user's choice (None = automatic from Windows).
/// VPN_LANGUAGE=pt|en forces a language (documentation screenshots).
pub fn aplicar(escolha: Option<Idioma>) {
    let forcado = std::env::var("VPN_LANGUAGE").ok().and_then(|c| Idioma::do_codigo(&c));
    let idioma = forcado.or(escolha).unwrap_or_else(do_windows);
    PORTUGUES.store(idioma == Idioma::Portugues, Ordering::Relaxed);
}

/// Windows interface language.
pub fn do_windows() -> Idioma {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    pelo_langid(unsafe { GetUserDefaultUILanguage() })
}

/// Windows LANGID -> app language: Portuguese (Brazil or Portugal)
/// uses pt-BR; all others, English.
fn pelo_langid(langid: u16) -> Idioma {
    const LANG_PORTUGUESE: u16 = 0x16;
    if langid & 0x3FF == LANG_PORTUGUESE {
        Idioma::Portugues
    } else {
        Idioma::Ingles
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idioma_pelo_windows() {
        assert_eq!(pelo_langid(0x0416), Idioma::Portugues); // pt-BR
        assert_eq!(pelo_langid(0x0816), Idioma::Portugues); // pt-PT
        assert_eq!(pelo_langid(0x0409), Idioma::Ingles); // en-US
        assert_eq!(pelo_langid(0x0C0A), Idioma::Ingles); // es-ES
        assert_eq!(pelo_langid(0x0407), Idioma::Ingles); // de-DE
    }

    #[test]
    fn codigos() {
        for i in [Idioma::Portugues, Idioma::Ingles] {
            assert_eq!(Idioma::do_codigo(i.codigo()), Some(i));
        }
        assert_eq!(Idioma::do_codigo("xx"), None);
    }

    #[test]
    fn macros_sem_inicializar_ficam_em_portugues() {
        let n = 2;
        assert_eq!(tr!("Conectar", "Connect"), "Conectar");
        assert_eq!(trf!("{n} contas", "{n} accounts"), "2 contas");
        assert_eq!(trf!("{} de {}", "{} of {}", 1, n), "1 de 2");
    }
}
