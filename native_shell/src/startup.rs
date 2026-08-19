//! HKCU Run key so the v6 process starts with Windows. User-scope only.

pub fn apply(enabled: bool) -> Result<(), String> {
    #[cfg(not(windows))]
    {
        let _ = enabled;
        Ok(())
    }
    #[cfg(windows)]
    {
        if cfg!(test) {
            return Ok(());
        }
        const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
        const NAME: &str = "HLSDownloader";
        if enabled {
            let exe = std::env::current_exe().map_err(|error| error.to_string())?;
            let value = format!("\"{}\"", exe.display());
            let status = std::process::Command::new("reg")
                .args(["add", KEY, "/v", NAME, "/t", "REG_SZ", "/d", &value, "/f"])
                .status()
                .map_err(|error| error.to_string())?;
            if status.success() {
                Ok(())
            } else {
                Err("写入开机启动失败".into())
            }
        } else {
            let _ = std::process::Command::new("reg")
                .args(["delete", KEY, "/v", NAME, "/f"])
                .status();
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn apply_is_noop_in_unit_tests() {
        super::apply(false).unwrap();
    }
}
