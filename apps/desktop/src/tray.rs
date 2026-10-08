use gpui_kit::*;
use poe2_service::{Command, ServiceHandle};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

pub struct TrayState {
    _icon: TrayIcon,
    _events: Task<()>,
}
impl Global for TrayState {}

fn tray_image(light_ink: bool) -> anyhow::Result<Icon> {
    #[cfg(target_os = "macos")]
    let (bytes, size): (&[u8], u32) = {
        let _ = light_ink;
        (include_bytes!("../assets/icons/tray-template-36.rgba"), 36)
    };
    #[cfg(not(target_os = "macos"))]
    let (bytes, size): (&[u8], u32) = (
        if light_ink {
            include_bytes!("../assets/icons/tray-light-32.rgba")
        } else {
            include_bytes!("../assets/icons/tray-dark-32.rgba")
        },
        32,
    );
    Ok(Icon::from_rgba(bytes.to_vec(), size, size)?)
}

#[cfg(target_os = "windows")]
fn taskbar_uses_light_ink() -> bool {
    // The taskbar follows SystemUsesLightTheme, independently of the app's
    // manually selected appearance and AppsUseLightTheme.
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
        .and_then(|key| key.get_value::<u32, _>("SystemUsesLightTheme"))
        .map(|light| light == 0)
        .unwrap_or(true)
}

pub fn install(services: ServiceHandle, cx: &mut App) -> anyhow::Result<()> {
    let menu = Menu::new();
    let show = MenuItem::new("打开 PoE Toolkit", true, None);
    let refresh = MenuItem::new("刷新行情", true, None);
    let pause = MenuItem::new("暂停自动刷新与补丁更新", true, None);
    let quit = MenuItem::new("退出", true, None);
    menu.append_items(&[
        &show,
        &refresh,
        &pause,
        &PredefinedMenuItem::separator(),
        &quit,
    ])?;
    #[cfg(target_os = "windows")]
    let mut light_ink = taskbar_uses_light_ink();
    #[cfg(not(target_os = "windows"))]
    let light_ink = false;
    let icon = TrayIconBuilder::new()
        .with_tooltip("PoE Toolkit")
        .with_menu(Box::new(menu))
        .with_icon(tray_image(light_ink)?)
        .with_icon_as_template(cfg!(target_os = "macos"))
        .build()?;
    #[cfg(target_os = "windows")]
    let appearance_icon = icon.clone();
    let events = cx.spawn(async move |cx| {
        #[cfg(target_os = "windows")]
        let mut last_appearance_check = std::time::Instant::now();
        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(250))
                .await;
            cx.update(|cx| {
                #[cfg(target_os = "windows")]
                if last_appearance_check.elapsed() >= std::time::Duration::from_secs(2) {
                    last_appearance_check = std::time::Instant::now();
                    let next = taskbar_uses_light_ink();
                    if next != light_ink
                        && let Ok(image) = tray_image(next)
                        && appearance_icon.set_icon(Some(image)).is_ok()
                    {
                        light_ink = next;
                    }
                }
                let state = services.snapshot();
                refresh.set_enabled(!state.busy && state.profile.market.is_some());
                pause.set_enabled(
                    !state.busy && (state.profile.auto_refresh || state.profile.auto_apply),
                );
                quit.set_enabled(!state.busy);
                while let Ok(event) = MenuEvent::receiver().try_recv() {
                    if event.id == *show.id() {
                        if let Some(window) = cx.windows().first().copied() {
                            let _ = window.update(cx, |_, window, _| window.activate_window());
                            cx.activate(true);
                        } else {
                            crate::open_main(services.clone(), cx);
                        }
                    } else if event.id == *refresh.id() && !state.busy {
                        let _ = services.send(Command::Refresh);
                    } else if event.id == *pause.id() && !state.busy {
                        let mut profile = state.profile.clone();
                        profile.auto_refresh = false;
                        profile.auto_apply = false;
                        let _ = services.send(Command::SaveProfile(profile.into()));
                    } else if event.id == *quit.id() && !state.busy {
                        crate::app::request_quit(&services, cx);
                    }
                }
            });
        }
    });
    cx.set_global(TrayState {
        _icon: icon,
        _events: events,
    });
    Ok(())
}
