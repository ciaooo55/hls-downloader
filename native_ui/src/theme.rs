use hls_client::CorePipeResponse;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub fn from_values(mode: &str) -> Self {
        match mode {
            "light" => Self::Light,
            "dark" => Self::Dark,
            _ => Self::System,
        }
    }

    pub fn from_response(response: Option<&CorePipeResponse>) -> Self {
        match response {
            Some(CorePipeResponse::Settings { theme_mode, .. }) => Self::from_values(theme_mode),
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
    fn theme_defaults_to_system_and_resolves_explicit_modes() {
        assert_eq!(ThemeMode::from_values(""), ThemeMode::System);
        assert_eq!(ThemeMode::from_values("invalid"), ThemeMode::System);
        assert_eq!(ThemeMode::from_response(None), ThemeMode::System);
        assert_eq!(ThemeMode::from_values("system"), ThemeMode::System);
        assert_eq!(ThemeMode::from_values("light"), ThemeMode::Light);
        assert_eq!(ThemeMode::from_values("dark"), ThemeMode::Dark);
        for dark in [false, true] {
            assert_eq!(ThemeMode::System.resolve(dark), dark);
            assert!(!ThemeMode::Light.resolve(dark));
            assert!(ThemeMode::Dark.resolve(dark));
        }
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

    #[test]
    fn light_theme_secondary_tokens_meet_wcag_aa_contrast() {
        use slint::ComponentHandle;

        fn relative_luminance(color: slint::Color) -> f64 {
            let linear = |channel: u8| {
                let s = f64::from(channel) / 255.0;
                if s <= 0.03928 {
                    s / 12.92
                } else {
                    ((s + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(color.red())
                + 0.7152 * linear(color.green())
                + 0.0722 * linear(color.blue())
        }

        fn contrast_ratio(fg: slint::Color, bg: slint::Color) -> f64 {
            let l1 = relative_luminance(fg);
            let l2 = relative_luminance(bg);
            let (hi, lo) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
            (hi + 0.05) / (lo + 0.05)
        }

        crate::offscreen::OffscreenPlatform::init_test();
        let window = crate::ConfirmWindow::new().unwrap();
        let tokens = window.global::<crate::Tokens>();
        tokens.invoke_apply_theme(false);

        let ink = tokens.get_ink();
        let muted = tokens.get_muted();
        let faint = tokens.get_faint();
        for surface in [
            tokens.get_surface(),
            tokens.get_surface_2(),
            tokens.get_canvas(),
        ] {
            assert!(
                contrast_ratio(muted, surface) >= 4.5,
                "light muted contrast too low: {:.2}",
                contrast_ratio(muted, surface)
            );
            assert!(
                contrast_ratio(faint, surface) >= 4.5,
                "light faint contrast too low: {:.2}",
                contrast_ratio(faint, surface)
            );
        }
        assert!(relative_luminance(ink) < relative_luminance(muted));
        assert!(relative_luminance(muted) < relative_luminance(faint));
    }
}
