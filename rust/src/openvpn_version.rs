//! The OpenVPN version on this computer and what to tell the user about it:
//! too old (Linux: the distribution's package, below the recommended
//! version) or older than the one the app brings (Windows: the embedded
//! installer can update it).

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Version {
            major,
            minor,
            patch,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// "OpenVPN 2.5.11 x86_64-pc-linux-gnu [SSL...]", "2.7.8-I002" or "2.6" ->
/// the version (the first one in the text).
pub fn parse(text: &str) -> Option<Version> {
    let word = text
        .split_whitespace()
        .find(|w| w.starts_with(|c: char| c.is_ascii_digit()))?;
    let mut numbers = word
        .split(|c: char| !c.is_ascii_digit())
        .map(|n| n.parse::<u32>().ok());
    let major = numbers.next()??;
    let minor = numbers.next()??;
    let patch = numbers.next().flatten().unwrap_or(0);
    Some(Version::new(major, minor, patch))
}

/// What the app shows about the installed OpenVPN.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Advice {
    Nothing,
    /// Linux: older than the recommended version (update it from the system).
    Outdated {
        installed: Version,
        recommended: Version,
    },
    /// Windows: the installer inside the app has a newer version.
    UpdateAvailable {
        installed: Version,
        bundled: Version,
    },
}

/// The advice for `installed` (None: OpenVPN not found or its version unknown).
pub fn advice(installed: Option<Version>) -> Advice {
    use crate::platform::{installer, openvpn};
    let Some(installed) = installed else {
        return Advice::Nothing;
    };
    if installer::is_available() {
        if let Some(bundled) = parse(installer::MSI_VERSION) {
            if bundled > installed {
                return Advice::UpdateAvailable { installed, bundled };
            }
        }
    }
    match openvpn::RECOMMENDED_MINIMUM {
        Some(recommended) if installed < recommended => Advice::Outdated {
            installed,
            recommended,
        },
        _ => Advice::Nothing,
    }
}

/// The version of the OpenVPN at `path` (it runs `openvpn --version`).
pub fn installed(path: &std::path::Path) -> Option<Version> {
    parse(&crate::platform::openvpn::version_output(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_version() {
        assert_eq!(
            parse("OpenVPN 2.5.11 x86_64-pc-linux-gnu [SSL (OpenSSL)] [LZO]"),
            Some(Version::new(2, 5, 11))
        );
        assert_eq!(
            parse("OpenVPN 2.7.5 [git:v2.7.5/b25bb2a8bda814ed] Windows [SSL (OpenSSL)]"),
            Some(Version::new(2, 7, 5))
        );
        assert_eq!(parse("2.7.8-I002"), Some(Version::new(2, 7, 8)));
        assert_eq!(parse("2.6"), Some(Version::new(2, 6, 0)));
        assert_eq!(parse("OpenVPN 2.6_git"), Some(Version::new(2, 6, 0)));
        assert_eq!(parse("nothing here"), None);
        assert!(Version::new(2, 5, 11) < Version::new(2, 6, 0));
        assert!(Version::new(2, 10, 0) > Version::new(2, 9, 9));
        assert_eq!(Version::new(2, 7, 8).to_string(), "2.7.8");
    }

    #[test]
    fn no_advice_without_a_version() {
        assert_eq!(advice(None), Advice::Nothing);
    }
}
