//! Only presentation preferences; never part of a patch profile.
use gpui_kit::*;
use serde::{Deserialize, Serialize};
use std::path::Path;
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct LayoutPreferences {
    pub width: f32,
    pub height: f32,
    pub inspector: f32,
    pub navigation_expanded: bool,
}
impl Default for LayoutPreferences {
    fn default() -> Self {
        Self {
            width: if cfg!(windows) { 1160. } else { 1080. },
            height: 740.,
            inspector: if cfg!(windows) { 330. } else { 310. },
            navigation_expanded: false,
        }
    }
}
impl LayoutPreferences {
    pub fn load(root: &Path) -> Self {
        let mut p = std::fs::read(root.join(format!("window-{}.json", std::env::consts::OS)))
            .ok()
            .and_then(|b| serde_json::from_slice::<Self>(&b).ok())
            .unwrap_or_default();
        p.width = p.width.clamp(860., 2400.);
        p.height = p.height.clamp(580., 1600.);
        p.inspector = p.inspector.clamp(270., 420.);
        p
    }
    pub fn save(&self, root: &Path) {
        if let Ok(bytes) = serde_json::to_vec(self) {
            let _ = std::fs::write(
                root.join(format!("window-{}.json", std::env::consts::OS)),
                bytes,
            );
        }
    }
}
impl crate::app::Toolkit {
    pub(crate) fn persist_layout(&self, window: &Window) {
        if self.settings_only {
            return;
        }
        let size = window.viewport_size();
        LayoutPreferences {
            width: f32::from(size.width),
            height: f32::from(size.height),
            inspector: self.inspector_width,
            navigation_expanded: self.nav_expanded,
        }
        .save(&self.state.data_dir);
    }
}
