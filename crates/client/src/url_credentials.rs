//! Password-free FTP URL representations for persistence and diagnostics.

pub fn map_ftp_passwords(
    text: &str,
    mut password: impl FnMut(&str, &str) -> Result<(), String>,
) -> Result<String, String> {
    let lower = text.to_ascii_lowercase();
    let mut output = String::new();
    let mut cursor = 0;
    while let Some(relative) = lower[cursor..].find("://") {
        let separator = cursor + relative;
        let start = lower[..separator]
            .rfind(|c: char| !c.is_ascii_alphabetic())
            .map_or(0, |index| index + 1);
        if !matches!(&lower[start..separator], "ftp" | "ftps" | "sftp") {
            output.push_str(&text[cursor..separator + 3]);
            cursor = separator + 3;
            continue;
        }
        let authority_start = separator + 3;
        let end = text[authority_start..]
            .find('/')
            .map_or(text.len(), |offset| authority_start + offset);
        let authority = &text[authority_start..end];
        if let Some(at) = authority.rfind('@') {
            if let Some(colon) = authority[..at].find(':') {
                let clean = format!(
                    "{}{}",
                    &text[start..authority_start + colon],
                    &authority[at..]
                );
                password(&clean, &authority[colon + 1..at])?;
                output.push_str(&text[cursor..authority_start + colon]);
                output.push_str(&authority[at..]);
                cursor = end;
                continue;
            }
        }
        output.push_str(&text[cursor..end]);
        cursor = end;
    }
    output.push_str(&text[cursor..]);
    Ok(output)
}

pub fn redact_ftp_passwords(text: &str) -> String {
    map_ftp_passwords(text, |_, _| Ok(())).expect("redaction cannot fail")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_passwords_in_urls_and_diagnostics() {
        assert_eq!(
            redact_ftp_passwords("FTP://u:p%40ss@host/a"),
            "FTP://u@host/a"
        );
        assert_eq!(
            redact_ftp_passwords("sftp://u:p@ss@host/a"),
            "sftp://u@host/a"
        );
        assert_eq!(
            redact_ftp_passwords("failed ftps://u:secret@host/a"),
            "failed ftps://u@host/a"
        );
        for plain in ["", "ftp://host/a", "sftp://user@host/a", "https://host/a"] {
            assert_eq!(redact_ftp_passwords(plain), plain);
        }
    }

    #[test]
    fn redacts_engine_accepted_userinfo_characters() {
        for secret in [
            "pa'ss", "pa\"ss", "pa ss", "pa?ss", "pa#ss", "密碼", "pa@ss",
        ] {
            let url = format!("ftp://alice:{secret}@host/file.bin");
            let mut captured = None;
            let clean = map_ftp_passwords(&url, |authority, password| {
                captured = Some((authority.to_string(), password.to_string()));
                Ok(())
            })
            .unwrap();
            assert_eq!(clean, "ftp://alice@host/file.bin");
            assert_eq!(captured, Some(("ftp://alice@host".into(), secret.into())));
        }
    }

    #[test]
    fn password_protection_errors_fail_closed() {
        assert_eq!(
            map_ftp_passwords("ftp://u:p@host/a", |_, _| Err("vault unavailable".into())),
            Err("vault unavailable".into())
        );
    }
}
