//! Small, interruptible transitions. Business state never waits for an animation.
use gpui_kit::{base::motion::Transition, *};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

/// Unlike a permanently mounted clipped region, a settled closed disclosure
/// removes its descendants from keyboard navigation and the accessibility tree.
#[derive(IntoElement)]
pub struct Disclosure {
    id: ElementId,
    open: bool,
    header: Vec<AnyElement>,
    content: Option<AnyElement>,
}

impl Disclosure {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            open: false,
            header: Vec::new(),
            content: None,
        }
    }
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }
    pub fn content(mut self, content: impl IntoElement) -> Self {
        self.content = Some(content.into_any_element());
        self
    }
}
impl ParentElement for Disclosure {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.header.extend(elements);
    }
}
impl RenderOnce for Disclosure {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let progress = base::transition(
            (self.id.clone(), "reveal"),
            if self.open { 1.0_f32 } else { 0.0 },
            transition(PAGE),
            window,
            cx,
        );
        let mut surface = div().children(self.header);
        if (self.open || progress > 0.001)
            && let Some(content) = self.content
        {
            surface = surface.child(base::MotionReveal::new(self.id, progress, content));
        }
        surface
    }
}

pub const PAGE: Duration = Duration::from_millis(180);
pub const CONTROL: Duration = Duration::from_millis(120);
pub const DRAWER_ENTER: Duration = Duration::from_millis(220);
pub const DRAWER_EXIT: Duration = Duration::from_millis(160);

/// Only navigation changes replay content motion; quotes, selection and service
/// notifications deliberately do not participate in this identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Scene {
    page: crate::navigation::Page,
    game: poe2_core::GameId,
    changes: bool,
}

impl Scene {
    pub(crate) fn new(
        page: crate::navigation::Page,
        game: poe2_core::GameId,
        changes: bool,
    ) -> Self {
        Self {
            page,
            game,
            changes: page == crate::navigation::Page::Patch && changes,
        }
    }
}

pub(crate) fn selection(
    id: impl Into<ElementId>,
    selected: bool,
    hovered: bool,
    active: bool,
    window: &mut Window,
    cx: &mut App,
) -> component::button::ButtonCustomVariant {
    let p = crate::theme::Palette::of(cx);
    let id = id.into();
    let background = if selected {
        if active && !crate::theme::MAC {
            p.accent.opacity(0.12)
        } else {
            p.selected
        }
    } else if hovered {
        p.selected
    } else {
        transparent_black()
    };
    let foreground = if selected && active && !crate::theme::MAC {
        p.accent
    } else {
        p.text
    };
    let background = base::transition(
        (id.clone(), "background"),
        background,
        transition(CONTROL),
        window,
        cx,
    );
    let foreground = base::transition(
        (id, "foreground"),
        foreground,
        transition(CONTROL),
        window,
        cx,
    );
    component::button::ButtonCustomVariant::new(cx)
        .color(background)
        .foreground(foreground)
        .hover(background)
        .active(background)
}

pub fn ease_out(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

pub fn transition(duration: Duration) -> Transition {
    Transition::new(duration).ease(ease_out)
}

/// This preference is shared by both games and all windows, independent of
/// patch profiles. System accessibility preferences always take precedence.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Preferences {
    reduce_motion: bool,
}

pub struct MotionSettings {
    path: PathBuf,
    preferences: Preferences,
    pub system_reduced: bool,
}
impl Global for MotionSettings {}

impl MotionSettings {
    pub fn reduced_by_user(&self) -> bool {
        self.preferences.reduce_motion
    }
}

pub fn init(data_dir: &Path, cx: &mut App) {
    let path = data_dir.join("ui-preferences.json");
    let preferences = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    cx.set_global(MotionSettings {
        path,
        preferences,
        system_reduced: system_reduced_motion(),
    });
    apply(cx);
}

pub fn sync_system(cx: &mut App) {
    let reduced = system_reduced_motion();
    if cx.global::<MotionSettings>().system_reduced != reduced {
        cx.update_global::<MotionSettings, _>(|settings, _| settings.system_reduced = reduced);
        apply(cx);
    }
}

pub fn set_reduced(reduced: bool, cx: &mut App) -> anyhow::Result<()> {
    let settings = cx.global::<MotionSettings>();
    let preferences = Preferences {
        reduce_motion: reduced,
    };
    // Write before changing the live setting, so an I/O failure cannot look saved.
    let temporary = settings.path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(&preferences)?)?;
    std::fs::rename(&temporary, &settings.path)?;
    cx.update_global::<MotionSettings, _>(|settings, _| settings.preferences = preferences);
    apply(cx);
    Ok(())
}

fn apply(cx: &mut App) {
    let settings = cx.global::<MotionSettings>();
    cx.set_reduce_motion(settings.system_reduced || settings.preferences.reduce_motion);
}

#[cfg(target_os = "macos")]
fn system_reduced_motion() -> bool {
    objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

#[cfg(target_os = "windows")]
fn system_reduced_motion() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let mut enabled: i32 = 1;
    // Read-only OS preference query; the buffer is a correctly sized BOOL.
    let result = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&mut enabled as *mut i32).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    result.is_ok() && enabled == 0
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn system_reduced_motion() -> bool {
    false
}
