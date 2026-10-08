//! Native window composition lives here; business views never access an HWND.
use crate::theme::{MAC, Palette};
use gpui_kit::{component::TitleBar, prelude::FluentBuilder, *};

pub const TITLE_HEIGHT: f32 = if MAC { 52. } else { 48. };
#[cfg(not(target_os = "macos"))]
pub const NAV_COMPACT: f32 = 52.;
#[cfg(not(target_os = "macos"))]
pub const NAV_EXPANDED: f32 = 208.;

pub fn options(title: &str, bounds: Bounds<Pixels>, minimum: Size<Pixels>) -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some(title.to_owned().into()),
            // AppKit keeps real traffic lights in our unified toolbar.
            traffic_light_position: Some(point(px(16.), px(19.))),
            ..TitleBar::title_bar_options()
        }),
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(minimum),
        window_background: if MAC {
            WindowBackgroundAppearance::Transparent
        } else {
            WindowBackgroundAppearance::MicaBackdrop
        },
        ..TitleBar::window_options()
    }
}

/// False means the shell must paint a solid, readable system-colored fallback.
pub fn initialize(window: &mut Window, cx: &App) -> bool {
    #[cfg(target_os = "windows")]
    if windows_backdrop::set_dark_mode(window, gpui_kit::component::Theme::global(cx).is_dark())
        .is_err()
    {
        window.set_background_appearance(WindowBackgroundAppearance::Opaque);
        return false;
    }
    #[cfg(not(target_os = "windows"))]
    let _ = cx;
    if crate::theme::solid_surface() {
        #[cfg(target_os = "macos")]
        macos_backdrop::disable(window);
        window.set_background_appearance(WindowBackgroundAppearance::Opaque);
        return false;
    }
    window.set_background_appearance(if MAC {
        WindowBackgroundAppearance::Transparent
    } else {
        WindowBackgroundAppearance::MicaBackdrop
    });
    #[cfg(target_os = "windows")]
    {
        let enabled = windows_backdrop::enable(window).is_ok();
        if !enabled {
            window.set_background_appearance(WindowBackgroundAppearance::Opaque);
        }
        enabled
    }
    #[cfg(target_os = "macos")]
    {
        let enabled = macos_backdrop::enable(window).is_some();
        if !enabled {
            window.set_background_appearance(WindowBackgroundAppearance::Opaque);
        }
        enabled
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = window;
        MAC
    }
}

#[cfg(target_os = "macos")]
mod macos_backdrop {
    use gpui_kit::Window;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
        NSVisualEffectState, NSVisualEffectView, NSWindowOrderingMode,
    };
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    pub(super) fn enable(window: &Window) -> Option<()> {
        let main_thread = MainThreadMarker::new()?;
        let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw()
        else {
            return None;
        };
        // SAFETY: GPUI exposes its live NSView, and initializes us synchronously on
        // the AppKit thread. We borrow it only for this call and retain no pointers.
        let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let content = view.window()?.contentView()?;
        for child in content.subviews() {
            if let Some(effect) = child.downcast_ref::<NSVisualEffectView>() {
                effect.setHidden(false);
                return Some(());
            }
        }
        // GPUI's BlurredView strips AppKit's adaptive tint in updateLayer. Use a
        // plain NSVisualEffectView so dark/light materials retain their contrast.
        // The content view retains this one material layer and releases it with
        // the window; repeated activation/appearance updates reuse the same view.
        let effect = NSVisualEffectView::new(main_thread);
        effect.setFrame(content.bounds());
        effect.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        effect.setMaterial(NSVisualEffectMaterial::Sidebar);
        effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        effect.setState(NSVisualEffectState::FollowsWindowActiveState);
        content.addSubview_positioned_relativeTo(&effect, NSWindowOrderingMode::Below, None);
        Some(())
    }

    pub(super) fn disable(window: &Window) {
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return;
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return;
        };
        // SAFETY: same live, main-thread NSView borrow as enable; no pointer escapes.
        let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        if let Some(content) = view.window().and_then(|window| window.contentView()) {
            for child in content.subviews() {
                if let Some(effect) = child.downcast_ref::<NSVisualEffectView>() {
                    effect.setHidden(true);
                }
            }
        }
    }
}

/// Only empty regions participate in window dragging. Controls never inherit
/// a Drag hitbox (Windows non-client hit testing takes precedence over clicks).
pub fn drag_space(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .h_full()
        .window_control_area(WindowControlArea::Drag)
        .when(MAC, |el| {
            el.on_mouse_down(MouseButton::Left, |event, window, _| {
                if event.click_count == 2 {
                    window.titlebar_double_click();
                } else {
                    window.start_window_move();
                }
            })
        })
}

pub fn title_bar(
    leading: impl IntoElement,
    content: impl IntoElement,
    window: &Window,
    p: Palette,
) -> AnyElement {
    let mut bar = div()
        .h(px(TITLE_HEIGHT))
        .flex_shrink_0()
        .flex()
        .items_center()
        .child(
            div()
                .min_w(px(if MAC { 166. } else { 110. }))
                .h_full()
                .flex_shrink_0()
                .flex()
                .items_center()
                .child(drag_space("title-leading-drag").w(px(if MAC { 80. } else { 16. })))
                .child(leading),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .items_center()
                .child(content),
        )
        .child(drag_space("title-main-drag").w(px(32.)).flex_shrink_0());
    #[cfg(target_os = "windows")]
    {
        use gpui_kit::component::{Icon, IconName};
        let foreground = if window.is_window_active() {
            p.text
        } else {
            p.muted
        };
        let controls = [
            ("minimize", WindowControlArea::Min, IconName::WindowMinimize),
            (
                "maximize",
                WindowControlArea::Max,
                if window.is_maximized() {
                    IconName::WindowRestore
                } else {
                    IconName::WindowMaximize
                },
            ),
            ("close", WindowControlArea::Close, IconName::WindowClose),
        ];
        bar = bar.children(controls.into_iter().map(|(id, area, icon)| {
            let close = id == "close";
            div()
                .id(id)
                .w(px(46.))
                .h_full()
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .window_control_area(area)
                .text_color(foreground)
                .hover(move |style| {
                    style
                        .bg(if close {
                            rgb(0xc42b1c).into()
                        } else {
                            p.selected
                        })
                        .text_color(if close {
                            rgb(0xffffff).into()
                        } else {
                            foreground
                        })
                })
                .child(Icon::new(icon).size(px(16.)))
        }));
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (&mut bar, window, p);
    }
    bar.into_any_element()
}

#[cfg(target_os = "windows")]
mod windows_backdrop {
    use gpui_kit::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::{
        Foundation::HWND,
        Graphics::Dwm::{
            DWMSBT_MAINWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
            DwmGetWindowAttribute, DwmSetWindowAttribute,
        },
    };

    pub(super) fn set_dark_mode(window: &Window, dark: bool) -> anyhow::Result<()> {
        let RawWindowHandle::Win32(handle) = HasWindowHandle::window_handle(window)
            .map_err(|error| anyhow::anyhow!("native window handle unavailable: {error}"))?
            .as_raw()
        else {
            anyhow::bail!("window has no HWND");
        };
        let hwnd = HWND(handle.hwnd.get() as *mut _);
        let dark: i32 = dark.into();
        // SAFETY: the live window owns HWND; DWM copies a correctly sized BOOL.
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark as *const _ as _,
                std::mem::size_of_val(&dark) as u32,
            )?;
        }
        Ok(())
    }

    pub(super) fn enable(window: &Window) -> anyhow::Result<()> {
        let RawWindowHandle::Win32(handle) = HasWindowHandle::window_handle(window)
            .map_err(|error| anyhow::anyhow!("native window handle unavailable: {error}"))?
            .as_raw()
        else {
            anyhow::bail!("window has no HWND");
        };
        let hwnd = HWND(handle.hwnd.get() as *mut _);
        let backdrop = DWMSBT_MAINWINDOW;
        let mut applied = 0i32;
        // SAFETY: HWND is borrowed from the live GPUI window. These synchronous
        // DWM calls copy fixed-size values; none of the pointers escape this scope.
        unsafe {
            // Calling the supported API also detects older Windows versions without
            // relying on a manifest-dependent OS-version check. Fail closed on error.
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                &backdrop as *const _ as _,
                std::mem::size_of_val(&backdrop) as u32,
            )?;
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                &mut applied as *mut _ as _,
                std::mem::size_of_val(&applied) as u32,
            )?;
            anyhow::ensure!(
                applied == backdrop.0,
                "DWM did not accept the Mica backdrop"
            );
            // GPUI already presents a transparent DirectComposition surface.
            // Extending a GDI glass frame here paints a second set of DWM caption
            // buttons over our non-client hit regions, so leave that frame alone.
        }
        Ok(())
    }
}
