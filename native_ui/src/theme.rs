use hls_client::CorePipeResponse;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub fn from_values(mode: &str, legacy_dark: Option<bool>) -> Self {
        match mode {
            "system" => Self::System,
            "light" => Self::Light,
            "dark" => Self::Dark,
            _ => match legacy_dark {
                Some(true) => Self::Dark,
                Some(false) => Self::Light,
                None => Self::System,
            },
        }
    }

    pub fn from_response(response: Option<&CorePipeResponse>) -> Self {
        match response {
            Some(CorePipeResponse::Settings {
                theme_mode,
                dark_mode,
                ..
            }) => Self::from_values(theme_mode, Some(*dark_mode)),
            _ => Self::System,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::System => Self::Light,
            Self::Light => Self::Dark,
            Self::Dark => Self::System,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn resolve(self, system_dark: bool) -> bool {
        match self {
            Self::System => system_dark,
            Self::Light => false,
            Self::Dark => true,
        }
    }

    pub fn is_dark(self) -> bool {
        self.resolve(self == Self::System && system_dark())
    }
}

#[cfg(windows)]
fn system_dark() -> bool {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let path: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
        .encode_utf16()
        .collect();
    let name: Vec<u16> = "AppsUseLightTheme\0".encode_utf16().collect();
    let mut value = 1_u32;
    let mut size = std::mem::size_of_val(&value) as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        )
    };
    result == 0 && value == 0
}

#[cfg(not(windows))]
fn system_dark() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_migrates_explicit_legacy_values_and_defaults_new_profiles_to_system() {
        assert_eq!(ThemeMode::from_values("", Some(true)), ThemeMode::Dark);
        assert_eq!(ThemeMode::from_values("", Some(false)), ThemeMode::Light);
        assert_eq!(ThemeMode::from_values("", None), ThemeMode::System);
        assert_eq!(ThemeMode::from_values("invalid", None), ThemeMode::System);
        assert_eq!(
            ThemeMode::from_values("invalid", Some(true)),
            ThemeMode::Dark
        );
    }

    #[test]
    fn explicit_theme_overrides_legacy_and_only_system_tracks_os_changes() {
        for dark in [false, true] {
            assert_eq!(
                ThemeMode::from_values("system", Some(dark)),
                ThemeMode::System
            );
            assert_eq!(ThemeMode::System.resolve(dark), dark);
            assert!(!ThemeMode::Light.resolve(dark));
            assert!(ThemeMode::Dark.resolve(dark));
        }
        assert_eq!(
            ThemeMode::from_values("light", Some(true)),
            ThemeMode::Light
        );
        assert_eq!(ThemeMode::from_values("dark", Some(false)), ThemeMode::Dark);
    }

    #[test]
    fn theme_cycle_includes_system() {
        assert_eq!(ThemeMode::System.next().next().next(), ThemeMode::System);
    }

    #[cfg(not(windows))]
    #[test]
    fn system_theme_falls_back_to_light_off_windows() {
        assert!(!ThemeMode::System.is_dark());
    }
}
