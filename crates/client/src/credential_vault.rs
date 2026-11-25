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
        if let Some(hex) = value.strip_prefix(PREFIX) {
            if hex.is_empty() {
                return Err("DPAPI credential payload is empty".into());
            }
            decode_hex(hex)?;
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

#[cfg(any(windows, test))]
fn zeroize_bytes(bytes: &mut [u8]) {
    for byte in bytes {
        // SAFETY: `byte` is a valid, aligned mutable reference into `bytes`.
        // `write_volatile` prevents dead-store elimination before buffer drop/free.
        unsafe {
            std::ptr::write_volatile(byte, 0);
        }
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

#[cfg(windows)]
struct InputZeroizeGuard(Vec<u8>);

#[cfg(windows)]
impl Drop for InputZeroizeGuard {
    fn drop(&mut self) {
        zeroize_bytes(&mut self.0);
    }
}

#[cfg(windows)]
struct DpapiBlobGuard {
    blob: windows_sys::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB,
    zeroize_on_drop: bool,
}

#[cfg(windows)]
impl DpapiBlobGuard {
    fn new(zeroize_on_drop: bool) -> Self {
        Self {
            blob: windows_sys::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB {
                cbData: 0,
                pbData: std::ptr::null_mut(),
            },
            zeroize_on_drop,
        }
    }

    fn as_slice(&self) -> Result<&[u8], String> {
        if self.blob.cbData == 0 {
            return Ok(&[]);
        }
        if self.blob.pbData.is_null() {
            return Err("DPAPI returned null buffer with non-zero length".into());
        }
        // SAFETY: `self.blob.pbData` was allocated by `CryptProtectData`/`CryptUnprotectData`,
        // verified non-null above, and remains valid for `self.blob.cbData` bytes until `Drop`.
        Ok(unsafe { std::slice::from_raw_parts(self.blob.pbData, self.blob.cbData as usize) })
    }
}

#[cfg(windows)]
impl Drop for DpapiBlobGuard {
    fn drop(&mut self) {
        if !self.blob.pbData.is_null() {
            if self.zeroize_on_drop && self.blob.cbData > 0 {
                // SAFETY: `pbData` is non-null and valid for `cbData` writable bytes allocated
                // by DPAPI (`LocalAlloc`) and exclusively owned by `self` during `Drop`.
                let buf = unsafe {
                    std::slice::from_raw_parts_mut(self.blob.pbData, self.blob.cbData as usize)
                };
                zeroize_bytes(buf);
            }
            // SAFETY: `pbData` was allocated by DPAPI via `LocalAlloc` and has not yet been freed;
            // nulling `pbData` ensures idempotence.
            unsafe {
                windows_sys::Win32::Foundation::LocalFree(self.blob.pbData.cast());
            }
            self.blob.pbData = std::ptr::null_mut();
            self.blob.cbData = 0;
        }
    }
}

#[cfg(windows)]
fn protect_windows(value: &[u8]) -> Result<String, String> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let cb_data = u32::try_from(value.len())
        .map_err(|_| "DPAPI credential payload is too large".to_string())?;
    let mut input = InputZeroizeGuard(value.to_vec());
    let source = CRYPT_INTEGER_BLOB {
        cbData: cb_data,
        pbData: input.0.as_mut_ptr(),
    };
    let mut output = DpapiBlobGuard::new(false);
    // SAFETY: `source` points to `input.0` which is live for `cb_data` bytes across the call;
    // optional entropy/prompt pointers are null; `&mut output.blob` is a valid writable `DATA_BLOB`
    // whose `pbData` (if any is allocated) is freed by `DpapiBlobGuard::drop`.
    let ok = unsafe {
        CryptProtectData(
            &source,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output.blob,
        )
    };
    if ok == 0 {
        // SAFETY: `GetLastError` reads thread-local Win32 error state with no pointer preconditions.
        let err = unsafe { GetLastError() };
        return Err(format!("CryptProtectData failed: {err}"));
    }
    let encrypted = output.as_slice()?;
    Ok(format!("{PREFIX}{}", encode_hex(encrypted)))
}

#[cfg(windows)]
fn unprotect_windows(value: &[u8]) -> Result<String, String> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let cb_data = u32::try_from(value.len())
        .map_err(|_| "DPAPI credential payload is too large".to_string())?;
    let mut input = value.to_vec();
    let source = CRYPT_INTEGER_BLOB {
        cbData: cb_data,
        pbData: input.as_mut_ptr(),
    };
    let mut output = DpapiBlobGuard::new(true);
    // SAFETY: `source` points to `input` which is live for `cb_data` bytes across the call;
    // optional description/entropy/prompt pointers are null; `&mut output.blob` is a valid
    // writable `DATA_BLOB` whose decrypted `pbData` is volatile-zeroized and freed by
    // `DpapiBlobGuard::drop` on both success and error paths.
    let ok = unsafe {
        CryptUnprotectData(
            &source,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output.blob,
        )
    };
    if ok == 0 {
        // SAFETY: `GetLastError` reads thread-local Win32 error state with no pointer preconditions.
        let err = unsafe { GetLastError() };
        return Err(format!("CryptUnprotectData failed: {err}"));
    }
    let plaintext = output.as_slice()?;
    std::str::from_utf8(plaintext)
        .map(str::to_owned)
        .map_err(|error| format!("DPAPI credential is not UTF-8: {error}"))
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
    fn rejects_malformed_stored_credentials_before_reuse() {
        for invalid in ["dpapi:", "dpapi:aaa", "dpapi:gg"] {
            assert!(CredentialVault.protect(invalid).is_err());
        }
    }

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

    #[test]
    fn zeroize_bytes_clears_sensitive_buffer_in_place() {
        let mut secret = *b"Authorization: Bearer top-secret";
        zeroize_bytes(&mut secret);
        assert!(secret.iter().all(|&byte| byte == 0));
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
