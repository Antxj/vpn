//! Leitura de QR Code (rqrr, 100% Rust) e extracao de seed/usuario.

use crate::totp::normalize_seed;
use percent_encoding::percent_decode_str;

/// Decodifica o primeiro QR legivel da imagem.
pub fn decode_qr(img: &image::DynamicImage) -> Option<String> {
    let gray = img.to_luma8();
    let (w, h) = (gray.width() as usize, gray.height() as usize);
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| {
        gray.get_pixel(x as u32, y as u32).0[0]
    });
    for grid in prepared.detect_grids() {
        if let Ok((_meta, content)) = grid.decode() {
            if !content.is_empty() {
                return Some(content);
            }
        }
    }
    None
}

#[derive(Debug, Default, PartialEq)]
pub struct QrData {
    pub seed: Option<String>,
    pub user: Option<String>,
}

fn pct(s: &str) -> String {
    percent_decode_str(s).decode_utf8_lossy().into_owned()
}

fn param<'a>(url: &'a url::Url, names: &[&str]) -> Option<String> {
    url.query_pairs()
        .find(|(k, _)| names.contains(&k.to_lowercase().as_str()))
        .map(|(_, v)| v.into_owned())
}

/// Formatos aceitos: otpauth://, URL com parametros, seed base32 crua.
pub fn parse_payload(text: &str) -> QrData {
    let t = text.trim();
    let mut seed_raw = String::new();
    let mut user = String::new();

    if t.to_lowercase().starts_with("otpauth://") {
        if let Ok(u) = url::Url::parse(t) {
            seed_raw = param(&u, &["secret"]).unwrap_or_default();
            // rotulo: caminho (as vezes host+caminho) percent-decodificado
            let label = pct(u.path().trim_matches('/'));
            let label = label.trim();
            user = match label.split_once(':') {
                Some((_issuer, nome)) => nome.trim().to_string(),
                None => label.to_string(),
            };
        }
    } else if t.contains("://") || t.contains('?') {
        if let Ok(u) = url::Url::parse(t) {
            seed_raw = param(&u, &["secret", "seed", "token"]).unwrap_or_default();
            user = param(&u, &["user", "username", "login", "account"]).unwrap_or_default();
        }
    } else {
        seed_raw = t.to_string();
    }

    let mut out = QrData::default();
    if let Some(seed) = normalize_seed(&seed_raw) {
        if seed.len() >= 8 {
            out.seed = Some(seed);
        }
    }
    if !user.is_empty() {
        out.user = Some(user);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

    #[test]
    fn otpauth_com_issuer() {
        let d = parse_payload(&format!(
            "otpauth://totp/Empresa%20VPN:usuario.teste?secret={SEED}&issuer=Empresa"
        ));
        assert_eq!(d.seed.as_deref(), Some(SEED));
        assert_eq!(d.user.as_deref(), Some("usuario.teste"));
    }

    #[test]
    fn otpauth_simples() {
        // formato comum: label so com o usuario
        let d = parse_payload(&format!("otpauth://totp/usuario.teste?secret={SEED}&issuer=Empresa"));
        assert_eq!(d.seed.as_deref(), Some(SEED));
        assert_eq!(d.user.as_deref(), Some("usuario.teste"));
    }

    #[test]
    fn url_generica() {
        let d = parse_payload(&format!(
            "https://vpn.exemplo.com.br/enroll?user=usuario.teste&seed={SEED}"
        ));
        assert_eq!(d.seed.as_deref(), Some(SEED));
        assert_eq!(d.user.as_deref(), Some("usuario.teste"));
    }

    #[test]
    fn seed_crua_e_invalidos() {
        let d = parse_payload("gezd gnbv gy3t qojq gezd gnbv gy3t qojq");
        assert_eq!(d.seed.as_deref(), Some(SEED));
        assert!(parse_payload("https://exemplo.com.br/pagina").seed.is_none());
        assert!(parse_payload("123!").seed.is_none());
    }

    #[test]
    fn decodifica_qr_gerado() {
        let url = format!("otpauth://totp/usuario.teste?secret={SEED}&issuer=Empresa");
        let code = qrcode::QrCode::new(url.as_bytes()).unwrap();
        let img_str = code
            .render::<char>()
            .quiet_zone(true)
            .module_dimensions(4, 4)
            .build();
        // constroi imagem em tons de cinza a partir do render texto
        let lines: Vec<&str> = img_str.lines().collect();
        let h = lines.len() as u32;
        let w = lines[0].chars().count() as u32;
        let mut gray = image::GrayImage::new(w, h);
        for (y, line) in lines.iter().enumerate() {
            for (x, ch) in line.chars().enumerate() {
                let v = if ch == ' ' { 255u8 } else { 0u8 };
                gray.put_pixel(x as u32, y as u32, image::Luma([v]));
            }
        }
        let dynimg = image::DynamicImage::ImageLuma8(gray);
        let text = decode_qr(&dynimg).expect("nao decodificou");
        assert_eq!(text, url);
        let d = parse_payload(&text);
        assert_eq!(d.user.as_deref(), Some("usuario.teste"));
        assert_eq!(d.seed.as_deref(), Some(SEED));
    }
}
