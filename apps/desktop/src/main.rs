#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod components;
mod draft;
mod inspector;
mod layout;
mod motion;
mod navigation;
mod overlay;
mod presentation;
mod shell;
mod theme;
mod tray;
mod update_control;
mod update_feedback;
mod views;
mod window_chrome;
mod workbench;
mod workspace;

use gpui_kit::component::Root;
use gpui_kit::*;
use poe2_service::{ServiceHandle, default_data_dir};

fn main() -> anyhow::Result<()> {
    poe2_service::app_updates::startup();
    let data_dir = std::env::var_os("POE2_TOOLKIT_DATA_DIR")
        .map(std::path::PathBuf::from)
        .map(Ok)
        .unwrap_or_else(default_data_dir)?;
    let services = ServiceHandle::start(&data_dir)?;
    services.send(poe2_service::Command::WatchApplicationUpdates)?;
    gpui_kit::application()
        .with_assets(assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            motion::init(&data_dir, cx);
            theme::init(&data_dir, cx);
            navigation::install(cx, services.clone());
            let closing_service = services.clone();
            cx.on_window_closed(move |cx, _| {
                if cx.windows().is_empty()
                    && !(cx.has_global::<tray::TrayState>()
                        && closing_service.snapshot().close_to_tray)
                {
                    cx.quit();
                }
            })
            .detach();
            open_main(services.clone(), cx);
            let tray_services = services.clone();
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
                cx.update(|cx| {
                    if let Err(error) = tray::install(tray_services, cx) {
                        eprintln!("托盘初始化失败：{error}");
                    }
                });
            })
            .detach();
        });
    Ok(())
}

fn open_main(services: ServiceHandle, cx: &mut App) {
    let layout = layout::LayoutPreferences::load(&services.snapshot().data_dir);
    let bounds = Bounds::centered(None, size(px(layout.width), px(layout.height)), cx);
    cx.spawn(async move |cx| {
        if let Err(error) = cx.open_window(
            window_chrome::options("PoE Toolkit", bounds, size(px(860.), px(580.))),
            move |window, cx| {
                let view = cx.new(|cx| app::Toolkit::new(services, false, window, cx));
                cx.new(|cx| Root::new(view, window, cx).bg(transparent_black()))
            },
        ) {
            eprintln!("无法打开窗口：{error}");
        }
    })
    .detach();
}
