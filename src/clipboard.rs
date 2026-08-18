use base64::{Engine as _, engine::general_purpose::STANDARD};

const MAX_OSC52_BYTES: usize = 100_000;

pub fn osc52_copy(text: &str) -> Option<Vec<u8>> {
    (text.len() <= MAX_OSC52_BYTES)
        .then(|| format!("\u{1b}]52;c;{}\u{7}", STANDARD.encode(text.as_bytes())).into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_multiline_unicode_without_losing_internal_text() {
        assert_eq!(
            String::from_utf8(osc52_copy("界\n🙂").unwrap()).unwrap(),
            "\u{1b}]52;c;55WMCvCfmYI=\u{7}"
        );
    }

    #[test]
    fn rejects_excessive_payloads_without_panicking() {
        assert!(osc52_copy(&"x".repeat(MAX_OSC52_BYTES + 1)).is_none());
    }
}
