//! Linux and macOS: local time, and the language of a locale name.

use crate::i18n::Idioma;
use crate::platform::DataHora;

/// "pt_BR.UTF-8", "pt-BR", "pt_PT" -> Portuguese; anything else -> English.
pub fn pelo_locale(locale: &str) -> Idioma {
    if locale.to_ascii_lowercase().starts_with("pt") {
        Idioma::Portugues
    } else {
        Idioma::Ingles
    }
}

pub fn agora() -> DataHora {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        let agora = libc::time(std::ptr::null_mut());
        libc::localtime_r(&agora, &mut tm);
    }
    DataHora {
        year: (tm.tm_year + 1900) as u16,
        month: (tm.tm_mon + 1) as u16,
        day: tm.tm_mday as u16,
        hour: tm.tm_hour as u16,
        minute: tm.tm_min as u16,
        second: tm.tm_sec as u16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idioma_pelo_locale() {
        assert_eq!(pelo_locale("pt_BR.UTF-8"), Idioma::Portugues);
        assert_eq!(pelo_locale("pt-BR"), Idioma::Portugues);
        assert_eq!(pelo_locale("pt_PT"), Idioma::Portugues);
        assert_eq!(pelo_locale("en_US.UTF-8"), Idioma::Ingles);
        assert_eq!(pelo_locale("C.UTF-8"), Idioma::Ingles);
        assert_eq!(pelo_locale(""), Idioma::Ingles);
    }
}
