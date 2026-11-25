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
    // SAFETY: `HKEY_CURRENT_USER` is a valid predefined registry pseudo-handle, `path` and `name`
    // are NUL-terminated UTF-16 strings outliving the call, `RRF_RT_REG_DWORD` restricts output
    // to a 4-byte `DWORD`, and `value`/`size` point to a writable 4-byte stack `u32`.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            std::ptr::from_mut(&mut value).cast(),
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

    #[test]
    fn both_themes_text_and_interactive_tokens_meet_wcag_aa_contrast() {
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

        for dark in [false, true] {
            let mode = if dark { "dark" } else { "light" };
            tokens.invoke_apply_theme(dark);

            let surfaces = [
                ("surface", tokens.get_surface()),
                ("surface-2", tokens.get_surface_2()),
                ("surface-3", tokens.get_surface_3()),
                ("canvas", tokens.get_canvas()),
                ("rail", tokens.get_rail()),
                ("dialog", tokens.get_dialog()),
            ];

            for (surface_name, bg) in surfaces {
                for (token_name, fg) in [
                    ("ink", tokens.get_ink()),
                    ("muted", tokens.get_muted()),
                    ("faint", tokens.get_faint()),
                ] {
                    let ratio = contrast_ratio(fg, bg);
                    assert!(
                        ratio >= 4.5,
                        "{mode} {token_name} on {surface_name} contrast too low: {ratio:.2}"
                    );
                }

                for (border_name, border) in [
                    ("control-border", tokens.get_control_border()),
                    ("border-strong", tokens.get_border_strong()),
                    ("focus-ring", tokens.get_focus_ring()),
                    ("accent", tokens.get_accent()),
                    ("done", tokens.get_done()),
                    ("warn", tokens.get_warn()),
                    ("fail", tokens.get_fail()),
                    ("info", tokens.get_info()),
                ] {
                    let ratio = contrast_ratio(border, bg);
                    assert!(
                        ratio >= 3.0,
                        "{mode} {border_name} on {surface_name} interactive contrast too low: {ratio:.2}"
                    );
                }
            }

            for (label, fg, bg) in [
                (
                    "on-accent/accent",
                    tokens.get_on_accent(),
                    tokens.get_accent(),
                ),
                ("accent/surface", tokens.get_accent(), tokens.get_surface()),
                (
                    "accent/selected",
                    tokens.get_accent(),
                    tokens.get_selected(),
                ),
                (
                    "done/success-surface",
                    tokens.get_done(),
                    tokens.get_success_surface(),
                ),
                ("done/surface", tokens.get_done(), tokens.get_surface()),
                (
                    "warn/warn-surface",
                    tokens.get_warn(),
                    tokens.get_warn_surface(),
                ),
                ("warn/surface", tokens.get_warn(), tokens.get_surface()),
                (
                    "fail/fail-surface",
                    tokens.get_fail(),
                    tokens.get_fail_surface(),
                ),
                ("fail/surface", tokens.get_fail(), tokens.get_surface()),
                ("info/info-bg", tokens.get_info(), tokens.get_info_bg()),
                ("info/surface", tokens.get_info(), tokens.get_surface()),
                (
                    "media-ink/media-bg",
                    tokens.get_media_ink(),
                    tokens.get_media_bg(),
                ),
                (
                    "archive-ink/archive-bg",
                    tokens.get_archive_ink(),
                    tokens.get_archive_bg(),
                ),
                (
                    "program-ink/program-bg",
                    tokens.get_program_ink(),
                    tokens.get_program_bg(),
                ),
                (
                    "document-ink/document-bg",
                    tokens.get_document_ink(),
                    tokens.get_document_bg(),
                ),
                (
                    "torrent-ink/torrent-bg",
                    tokens.get_torrent_ink(),
                    tokens.get_torrent_bg(),
                ),
            ] {
                let ratio = contrast_ratio(fg, bg);
                assert!(ratio >= 4.5, "{mode} {label} contrast too low: {ratio:.2}");
            }
        }
    }
}
