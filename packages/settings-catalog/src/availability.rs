//! Where a Settings row exists and what a setting defaults to on one platform. The desktop's native
//! Settings and its search, `ghostex settings` (gxserver) and the generated Help files all ask here, so a
//! row that does nothing on a platform disappears from every surface at once.
//!
//! CDXC:Theming 2026-10-04 DECISION:
//! User: "let's make this not appear on windows because it's not changeable it looks like", then chose to hide the Blur slider on Windows only while "What shows behind the glass" is Desktop and windows (the system draws that blur and has no radius to set), and to hide the macOS-only Menu blur slider on Windows and Linux everywhere it is listed. Blur stays on Windows for Wallpaper only, Custom image and Live, where it blurs the picture.
//! SEE-ALSO: apps/desktop/src/app/window/settings_modal/store.rs, apps/desktop/src/app/window/settings_modal/tabs/theme/transparency.rs, server/src/ghostex_cli/settings.rs.
//!
//! CDXC:Theming 2026-10-10 DECISION:
//! User: "please disable transparency by default on windows to make the app faster by default for users" (2026-10-04), then "turn blur off by default for windows in the setup" (2026-10-10). Enable transparency (`windowGlass`) defaults to Never (`opaque`) on Windows, in Settings and in the first-run setup's transparency switch, which starts off; macOS and Linux keep Dark only. A saved choice always wins.
//! SEE-ALSO: apps/desktop/src/app/helpers/window_glass.rs (`refresh_window_glass`), apps/desktop/src/app/window/onboarding/model.rs (the setup's starting value).

use crate::json::J;
use crate::Platform;

/// A row that exists on some platforms only, or that is hidden on one platform while another setting has
/// a given value.
struct RowAvailability {
    key: &'static str,
    /// Platforms with the row; empty means all of them.
    only_on: &'static [Platform],
    /// `(platform, key, value)`: hidden on `platform` while setting `key` holds `value`.
    hidden_while: Option<(Platform, &'static str, &'static str)>,
}

const ROWS: &[RowAvailability] = &[
    RowAvailability {
        key: "windowGlassBlurRadius",
        only_on: &[],
        hidden_while: Some((
            Platform::Windows,
            "windowGlassSource",
            "desktopAndWindows",
        )),
    },
    RowAvailability {
        key: "windowGlassMenuBlurRadius",
        only_on: &[Platform::MacOs],
        hidden_while: None,
    },
];

/// A default that differs from `DEFAULT_GHOSTEX_SETTINGS` on one platform.
const PLATFORM_DEFAULTS: &[(Platform, &str, J)] = &[(Platform::Windows, "windowGlass", J::Str("opaque"))];

/// The platform as a customer writes it.
pub fn platform_name(platform: Platform) -> &'static str {
    match platform {
        Platform::MacOs => "macOS",
        Platform::Windows => "Windows",
        Platform::Linux => "Linux",
    }
}

/// The default of `key` on `platform` when it differs from the shared default.
pub fn platform_default(platform: Platform, key: &str) -> Option<&'static J> {
    PLATFORM_DEFAULTS
        .iter()
        .find(|(owner, name, _)| *owner == platform && *name == key)
        .map(|(_, _, value)| value)
}

/// The default of `key` on `platform`: its platform default, else the shared one.
pub fn default_value_on(platform: Platform, key: &str) -> Option<&'static J> {
    platform_default(platform, key).or_else(|| crate::default_value(key))
}

/// Every platform default of `key` that differs from the shared one, for the generated files.
pub fn platform_defaults(key: &str) -> Vec<(Platform, &'static J)> {
    PLATFORM_DEFAULTS
        .iter()
        .filter(|(_, name, _)| *name == key)
        .map(|(platform, _, value)| (*platform, value))
        .collect()
}

/// Whether the row of setting `key` is left out on `platform`. `read` gives the saved value of another
/// setting as text; a setting it does not know falls back to the platform's default.
pub fn row_hidden(platform: Platform, key: &str, read: impl Fn(&str) -> Option<String>) -> bool {
    if crate::built_in_extensions::key_unavailable_on(platform, key) {
        return true;
    }
    let Some(row) = ROWS.iter().find(|row| row.key == key) else {
        return false;
    };
    if !row.only_on.is_empty() && !row.only_on.contains(&platform) {
        return true;
    }
    row.hidden_while
        .is_some_and(|(owner, other_key, hidden_value)| {
            owner == platform
                && read(other_key)
                    .or_else(|| {
                        default_value_on(platform, other_key)
                            .and_then(J::as_str)
                            .map(str::to_string)
                    })
                    .as_deref()
                    == Some(hidden_value)
        })
}

/// Every row key [`row_hidden`] leaves out on `platform` for the current values.
pub fn hidden_row_keys(
    platform: Platform,
    read: impl Fn(&str) -> Option<String>,
) -> Vec<&'static str> {
    ROWS.iter()
        .map(|row| row.key)
        .filter(|key| row_hidden(platform, key, &read))
        .collect()
}

/// One sentence for the generated files when a row is not available everywhere.
pub fn availability_note(key: &str) -> Option<String> {
    if let Some(id) = crate::built_in_extensions::feature_switched_by(key) {
        if let Some(note) = crate::built_in_extensions::availability_note(id) {
            return Some(note);
        }
    }
    let row = ROWS.iter().find(|row| row.key == key)?;
    if !row.only_on.is_empty() {
        let names: Vec<&str> = row.only_on.iter().map(|p| platform_name(*p)).collect();
        return Some(format!("Only on {}.", names.join(" and ")));
    }
    let (platform, _, _) = row.hidden_while?;
    Some(format!(
        "Not shown on {} while What shows behind the glass is Desktop and windows.",
        platform_name(platform)
    ))
}
