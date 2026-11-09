//! Windows DPAPI-backed credential blobs for browser replay contexts.

const PREFIX: &str = "dpapi:";

#[derive(Debug, Default, Clone, Copy)]
pub struct CredentialVault;

impl CredentialVault {
    /// DPAPI on Windows; other platforms refuse so credential-bearing flows
    /// fail closed instead of storing plaintext.
    pub fn protect(&self, value: &str) -> Result<String, String> {
        if value.is_empty() {
            return Ok(String::new());
        }
        if value.starts_with(PREFIX) {
            return Ok(value.to_string());
        }
        #[cfg(windows)]
        {
            protect_windows(value.as_bytes())
        }
        #[cfg(not(windows))]
        {
            let _ = value;
            Err("DPAPI credentials require Windows".into())
        }
    }

    pub fn unprotect(&self, value: &str) -> Result<String, String> {
        if value.is_empty() || !value.starts_with(PREFIX) {
            return Ok(value.to_string());
        }
        #[cfg(windows)]
        {
            unprotect_windows(&decode_hex(&value[PREFIX.len()..])?)
        }
        #[cfg(not(windows))]
        {
            let _ = value;
            Err("DPAPI credentials require Windows".into())
        }
    }
}

#[cfg(windows)]
fn protect_windows(value: &[u8]) -> Result<String, String> {
    use windows_sys::Win32::Foundation::{GetLastError, LocalFree};
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let mut input = value.to_vec();
    let source = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptProtectData(
            &source,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(format!("CryptProtectData failed: {}", unsafe {
            GetLastError()
        }));
    }
    let encrypted = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) };
    let encoded = format!("{PREFIX}{}", encode_hex(encrypted));
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(encoded)
}

#[cfg(windows)]
fn unprotect_windows(value: &[u8]) -> Result<String, String> {
    use windows_sys::Win32::Foundation::{GetLastError, LocalFree};
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let mut input = value.to_vec();
    let source = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptUnprotectData(
            &source,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(format!("CryptUnprotectData failed: {}", unsafe {
            GetLastError()
        }));
    }
    let plaintext = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) };
    let value = String::from_utf8(plaintext.to_vec())
        .map_err(|error| format!("DPAPI credential is not UTF-8: {error}"));
    unsafe {
        LocalFree(output.pbData.cast());
    }
    value
}

fn encode_hex(value: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(value.len() * 2);
    for byte in value {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0x0f) as usize] as char);
    }
    result
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("DPAPI credential hex has odd length".into());
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().as_chunks::<2>().0 {
        let high =
            hex_digit(pair[0]).ok_or_else(|| "DPAPI credential hex is invalid".to_string())?;
        let low =
            hex_digit(pair[1]).ok_or_else(|| "DPAPI credential hex is invalid".to_string())?;
        bytes.push((high << 4) | low);
    }
    Ok(bytes)
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_plain_values_are_stable() {
        let vault = CredentialVault;
        assert_eq!(vault.protect("").unwrap(), "");
        #[cfg(not(windows))]
        assert!(vault.protect("secret").is_err());
    }

    #[test]
    fn hex_codec_roundtrips() {
        let value = b"credential\0blob";
        assert_eq!(decode_hex(&encode_hex(value)).unwrap(), value);
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_roundtrip_is_user_bound() {
        let vault = CredentialVault;
        let protected = vault.protect("session=secret").unwrap();
        assert!(protected.starts_with(PREFIX));
        assert_eq!(vault.unprotect(&protected).unwrap(), "session=secret");
    }
}
