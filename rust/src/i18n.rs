//! Idioma da interface: portugues (Brasil) ou ingles.
//!
//! Padrao: o idioma do Windows (portugues -> pt-BR; qualquer outro ->
//! ingles). O usuario pode fixar um dos dois na tela de contas.
//!
//! Os textos ficam no proprio codigo, lado a lado:
//! `tr!("Conectar", "Connect")` devolve &'static str e
//! `trf!("{n} de {total}", "{n} of {total}")` monta uma String (format!).

use std::sync::atomic::{AtomicBool, Ordering};

/// Texto fixo no idioma atual.
macro_rules! tr {
    ($pt:literal, $en:literal $(,)?) => {
        if $crate::i18n::pt() {
            $pt
        } else {
            $en
        }
    };
}

/// Texto formatado (como format!) no idioma atual.
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
    /// Codigo gravado nas configuracoes.
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

    /// Nome do idioma escrito nele mesmo (para o seletor).
    pub fn nome(self) -> &'static str {
        match self {
            Idioma::Portugues => "Português",
            Idioma::Ingles => "English",
        }
    }
}

// Sem inicializar (testes), fica em portugues.
static PORTUGUES: AtomicBool = AtomicBool::new(true);

pub fn pt() -> bool {
    PORTUGUES.load(Ordering::Relaxed)
}

/// Aplica a escolha do usuario (None = automatico pelo Windows).
/// VPN_IDIOMA=pt|en força um idioma (capturas de tela da documentacao).
pub fn aplicar(escolha: Option<Idioma>) {
    let forcado = std::env::var("VPN_IDIOMA").ok().and_then(|c| Idioma::do_codigo(&c));
    let idioma = forcado.or(escolha).unwrap_or_else(do_windows);
    PORTUGUES.store(idioma == Idioma::Portugues, Ordering::Relaxed);
}

/// Idioma da interface do Windows.
pub fn do_windows() -> Idioma {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    pelo_langid(unsafe { GetUserDefaultUILanguage() })
}

/// LANGID do Windows -> idioma do app: portugues (Brasil ou Portugal)
/// usa pt-BR; os demais, ingles.
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
