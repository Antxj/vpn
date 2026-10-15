//! Windows: interface language and local time of the system.

use crate::i18n::Idioma;
use crate::platform::DataHora;

/// Windows interface language: Portuguese (Brazil or Portugal) uses pt-BR;
/// all others, English.
pub fn idioma() -> Idioma {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    pelo_langid(unsafe { GetUserDefaultUILanguage() })
}

fn pelo_langid(langid: u16) -> Idioma {
    const LANG_PORTUGUESE: u16 = 0x16;
    if langid & 0x3FF == LANG_PORTUGUESE {
        Idioma::Portugues
    } else {
        Idioma::Ingles
    }
}

/// SYSTEMTIME (GetLocalTime).
#[repr(C)]
#[derive(Default)]
struct SystemTimeW {
    year: u16,
    month: u16,
    dow: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    ms: u16,
}

pub fn agora() -> DataHora {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLocalTime(t: *mut SystemTimeW);
    }
    let mut t = SystemTimeW::default();
    unsafe { GetLocalTime(&mut t) };
    DataHora {
        year: t.year,
        month: t.month,
        day: t.day,
        hour: t.hour,
        minute: t.minute,
        second: t.second,
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
}
