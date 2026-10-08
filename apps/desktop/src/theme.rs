//! The only place where OS appearance and component-library tokens are adapted.
use gpui_kit::{
    component::{Theme, ThemeMode, ThemeTokens},
    *,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "跟随系统",
            Self::Light => "浅色",
            Self::Dark => "深色",
        }
    }

    fn window_override(self) -> Option<WindowAppearance> {
        match self {
            Self::System => None,
            Self::Light => Some(WindowAppearance::Light),
            Self::Dark => Some(WindowAppearance::Dark),
        }
    }

    fn resolve(self, system: WindowAppearance) -> ThemeMode {
        self.window_override().unwrap_or(system).into()
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct AppearancePreferences {
    appearance: Appearance,
}

/// App-wide preference, independent of game profiles and motion preferences.
pub struct AppearanceSettings {
    path: PathBuf,
    preference: AppearancePreferences,
}
impl Global for AppearanceSettings {}

impl AppearanceSettings {
    fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("appearance.json");
        let preference = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { path, preference }
    }

    pub fn appearance(&self) -> Appearance {
        self.preference.appearance
    }

    fn save(&mut self, appearance: Appearance) -> anyhow::Result<()> {
        let preference = AppearancePreferences { appearance };
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(&preference)?)?;
        std::fs::rename(&temporary, &self.path)?;
        // An unsuccessful write must preserve both the live and saved choice.
        self.preference = preference;
        Ok(())
    }
}

pub fn init(data_dir: &Path, cx: &mut App) {
    let settings = AppearanceSettings::load(data_dir);
    cx.set_window_appearance(settings.appearance().window_override());
    cx.set_global(settings);
    sync(None, cx);
}

pub fn set_appearance(
    appearance: Appearance,
    window: &mut Window,
    cx: &mut App,
) -> anyhow::Result<()> {
    cx.update_global::<AppearanceSettings, _>(|settings, _| settings.save(appearance))?;
    // GPUI applies this to NSApplication, including existing and future windows.
    // Windows needs the explicit DWM integration in window_chrome as well.
    cx.set_window_appearance(appearance.window_override());
    sync(Some(window), cx);
    Ok(())
}

pub const MAC: bool = cfg!(target_os = "macos");
pub const CONTROL_HEIGHT: f32 = if MAC { 28. } else { 32. };
#[cfg(not(target_os = "macos"))]
pub const NAV_HEIGHT: f32 = if MAC { 32. } else { 38. };

#[derive(Clone, Copy)]
pub struct Palette {
    pub canvas: Hsla,
    pub sidebar: Hsla,
    pub surface: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub border: Hsla,
    pub accent: Hsla,
    pub selected: Hsla,
    pub success: Hsla,
    pub danger: Hsla,
    pub danger_bg: Hsla,
}
impl Palette {
    pub fn of(cx: &App) -> Self {
        let palette = Self::for_mode(Theme::global(cx).is_dark());
        if cx.has_global::<SystemSurface>() {
            Self::contrast(palette, cx.global::<SystemSurface>())
        } else {
            palette
        }
    }
    fn for_mode(dark: bool) -> Self {
        let colors = if dark {
            [
                0x202020,
                0x292929,
                0x2d2d2d,
                0xf2f2f2,
                0xababaf,
                0x414144,
                if MAC { 0x0a84ff } else { 0x60b8ff },
                0x38383b,
                0x75cf91,
                0xffa4a4,
                0x3b292b,
            ]
        } else {
            [
                if MAC { 0xf5f5f5 } else { 0xf3f3f3 },
                if MAC { 0xebebed } else { 0xf3f3f3 },
                0xffffff,
                0x202124,
                0x636368,
                0xdedee2,
                if MAC { 0x007aff } else { 0x0067c0 },
                0xe3e3e7,
                0x237a40,
                0xb42332,
                0xffeeee,
            ]
        }
        .map(|c| rgb(c).into());
        let [
            canvas,
            sidebar,
            surface,
            text,
            muted,
            border,
            accent,
            selected,
            success,
            danger,
            danger_bg,
        ] = colors;
        Self {
            canvas,
            sidebar,
            surface,
            text,
            muted,
            border,
            accent,
            selected,
            success,
            danger,
            danger_bg,
        }
    }
}

pub fn sync(window: Option<&mut Window>, cx: &mut App) {
    let preference = cx
        .try_global::<AppearanceSettings>()
        .map(AppearanceSettings::appearance)
        .unwrap_or_default();
    // A window's cached appearance can still describe the previous override
    // while switching back to System. Resolve from the application's source.
    Theme::change(preference.resolve(cx.window_appearance()), window, cx);
    cx.set_global(SystemSurface::read());
    let p = Palette::of(cx);
    let contrast_foreground = cx.global::<SystemSurface>().colors.map(|colors| colors[3]);
    let t = Theme::global_mut(cx);
    t.font_family = if MAC { ".SystemUIFont" } else { "Segoe UI" }.into();
    t.font_size = px(if MAC { 13. } else { 14. });
    t.radius = px(if MAC { 6. } else { 4. });
    t.radius_lg = px(8.);
    t.motion.duration_fast = crate::motion::CONTROL;
    t.motion.duration_normal = crate::motion::PAGE;
    t.motion.duration_slow = crate::motion::DRAWER_ENTER;
    t.motion.spring_move = base::Spring::new(crate::motion::DRAWER_ENTER)
        .with_damping(1.0)
        .with_epsilon(0.1);
    t.background = p.surface;
    t.foreground = p.text;
    t.muted = p.canvas;
    t.muted_foreground = p.muted;
    t.border = p.border;
    t.input = p.border;
    t.ring = p.accent;
    t.primary = p.accent;
    t.primary_foreground = contrast_foreground.unwrap_or_else(|| {
        rgb(if !MAC && t.is_dark() {
            0x18202a
        } else {
            0xffffff
        })
        .into()
    });
    t.button = p.surface;
    t.button_foreground = p.text;
    t.button_hover = p.selected;
    t.button_active = p.border;
    t.button_primary = p.accent;
    t.button_primary_hover = p.accent;
    t.button_primary_active = p.accent;
    t.button_primary_foreground = t.primary_foreground;
    t.colors.list = p.surface;
    t.list_hover = p.canvas;
    t.list_active = p.selected;
    t.list_active_border = p.accent;
    t.table = p.surface;
    t.table_head = p.canvas;
    t.table_head_foreground = p.muted;
    t.table_hover = p.canvas;
    // GPUI Component also uses this token as a layer above cell text.
    // Keep it translucent so any selection mode preserves readable content.
    t.table_active = p.accent.opacity(if t.is_dark() { 0.18 } else { 0.12 });
    t.table_active_border = p.accent;
    t.table_even = p.canvas;
    t.table_row_border = p.border;
    t.switch = p.border;
    t.switch_thumb = rgb(0xffffff).into();
    t.tokens = ThemeTokens::from(t.colors);
    Theme::sync_base(cx);
    cx.refresh_windows();
}

/// System settings are sampled on activation/appearance changes, not per table cell.
struct SystemSurface {
    solid: bool,
    colors: Option<[Hsla; 4]>,
}
impl Global for SystemSurface {}
impl SystemSurface {
    fn read() -> Self {
        #[cfg(target_os = "windows")]
        {
            use windows::Win32::{
                Graphics::Gdi::{
                    COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT,
                    GetSysColor,
                },
                UI::{
                    Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW},
                    WindowsAndMessaging::{
                        SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
                        SystemParametersInfoW,
                    },
                },
            };
            let mut hc = HIGHCONTRASTW {
                cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
                ..Default::default()
            };
            // SAFETY: synchronous read-only query into an initialized, correctly sized buffer.
            let contrast = unsafe {
                SystemParametersInfoW(
                    SPI_GETHIGHCONTRAST,
                    hc.cbSize,
                    Some((&mut hc as *mut HIGHCONTRASTW).cast()),
                    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                )
            }
            .is_ok()
                && (hc.dwFlags & HCF_HIGHCONTRASTON) == HCF_HIGHCONTRASTON;
            let colors = contrast.then(|| {
                [
                    COLOR_WINDOW,
                    COLOR_WINDOWTEXT,
                    COLOR_HIGHLIGHT,
                    COLOR_HIGHLIGHTTEXT,
                ]
                .map(|index| {
                    // COLORREF is 0x00bbggrr, unlike GPUI's RGB literal.
                    let c = unsafe { GetSysColor(index) };
                    rgb(((c & 255) << 16) | (c & 0xff00) | ((c >> 16) & 255)).into()
                })
            });
            let transparent = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
                .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
                .and_then(|key| key.get_value::<u32, _>("EnableTransparency"))
                .unwrap_or(1)
                != 0;
            Self {
                solid: contrast || !transparent,
                colors,
            }
        }
        #[cfg(target_os = "macos")]
        {
            let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
            Self {
                solid: workspace.accessibilityDisplayShouldReduceTransparency()
                    || workspace.accessibilityDisplayShouldIncreaseContrast(),
                colors: None,
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Self {
                solid: true,
                colors: None,
            }
        }
    }
}
impl Palette {
    fn contrast(mut p: Self, system: &SystemSurface) -> Self {
        if let Some([background, text, highlight, _]) = system.colors {
            p.canvas = background;
            p.sidebar = background;
            p.surface = background;
            p.text = text;
            p.muted = text;
            p.border = text;
            p.accent = highlight;
            p.selected = background;
            p.success = text;
            p.danger = text;
            p.danger_bg = background;
        }
        p
    }
}
pub fn solid_surface() -> bool {
    SystemSurface::read().solid
}
pub fn surface_is_solid(cx: &App) -> bool {
    cx.has_global::<SystemSurface>() && cx.global::<SystemSurface>().solid
}
