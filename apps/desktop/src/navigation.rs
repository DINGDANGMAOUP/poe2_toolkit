#[cfg(not(target_os = "macos"))]
use gpui_kit::component::IconName;
use gpui_kit::*;

actions!(
    toolkit,
    [
        Market,
        Patch,
        History,
        Settings,
        Refresh,
        Search,
        ClosePanel,
        CheckForUpdates,
        Quit
    ]
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Market,
    Patch,
    History,
    Settings,
}
impl Page {
    pub const MAIN: [Self; 3] = [Self::Patch, Self::Market, Self::History];
    pub fn title(self) -> &'static str {
        match self {
            Self::Market => "行情",
            Self::Patch => "补丁",
            Self::History => "记录",
            Self::Settings => "设置",
        }
    }
    #[cfg(not(target_os = "macos"))]
    pub fn icon(self) -> IconName {
        match self {
            Self::Market => IconName::ChartPie,
            Self::Patch => IconName::FileText,
            Self::History => IconName::Undo2,
            Self::Settings => IconName::Settings,
        }
    }
}

pub fn install(cx: &mut App, services: poe2_service::ServiceHandle) {
    let modifier = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    cx.bind_keys([
        KeyBinding::new("escape", ClosePanel, Some("Toolkit")),
        KeyBinding::new(&format!("{modifier}-1"), Patch, Some("Toolkit")),
        KeyBinding::new(&format!("{modifier}-2"), Market, Some("Toolkit")),
        KeyBinding::new(&format!("{modifier}-3"), History, Some("Toolkit")),
        KeyBinding::new(&format!("{modifier}-,"), Settings, Some("Toolkit")),
        KeyBinding::new(&format!("{modifier}-r"), Refresh, Some("Toolkit")),
        KeyBinding::new(&format!("{modifier}-f"), Search, Some("Toolkit")),
    ]);
    cx.on_action(move |_: &Quit, cx| {
        // Global actions may still be inside the focused window's update.
        // Defer before borrowing that window to present its discard dialog.
        let services = services.clone();
        cx.defer(move |cx| crate::app::request_quit(&services, cx));
    });
    if cfg!(target_os = "macos") {
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.set_menus(vec![
            Menu {
                disabled: false,
                name: "PoE Toolkit".into(),
                items: vec![
                    MenuItem::action("设置…", Settings),
                    MenuItem::action("检查更新…", CheckForUpdates),
                    MenuItem::separator(),
                    MenuItem::action("退出 PoE Toolkit", Quit),
                ],
            },
            Menu {
                disabled: false,
                name: "显示".into(),
                items: vec![
                    MenuItem::action("补丁", Patch),
                    MenuItem::action("行情", Market),
                    MenuItem::action("操作记录", History),
                ],
            },
        ]);
    }
}
