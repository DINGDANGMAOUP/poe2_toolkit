#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod windows;
use crate::{
    app::Toolkit,
    components::*,
    motion,
    navigation::{self, Page},
    overlay::Intent,
    theme::*,
    window_chrome,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    component::{
        Disableable, Icon, IconName, Root, Selectable, Side, Sizable, WindowExt,
        button::{Button, ButtonVariants},
        menu::{DropdownMenu, PopupMenuItem},
        spinner::Spinner,
    },
    *,
};
use poe2_service::{Command, update_activity::UpdateTarget};

impl Toolkit {
    fn game_switcher(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::of(cx);
        let current = self.state.profile.game;
        let owner = cx.entity().downgrade();

        Button::new("game-switcher")
            .ghost()
            .accessibility_label(format!("切换游戏，当前 {}", current.label()))
            .h(px(CONTROL_HEIGHT))
            .px_2()
            .rounded(px(6.))
            .disabled(self.busy() || window.has_active_dialog(cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(17.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(current.label()),
                    )
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .size(px(16.))
                            .text_color(p.muted),
                    ),
            )
            .dropdown_menu(move |mut menu, _, _| {
                menu = menu.min_w(px(280.)).max_w(px(280.)).check_side(Side::Right);
                for (index, game) in poe2_core::GameId::ALL.into_iter().enumerate() {
                    let owner = owner.clone();
                    let selected = current == game;
                    let description = if game.is_poe2() {
                        "Path of Exile 2"
                    } else {
                        "Path of Exile"
                    };
                    menu = menu.item(
                        PopupMenuItem::element(move |_, cx| {
                            div()
                                .id(("game-option", index))
                                .role(Role::Group)
                                .aria_label(format!(
                                    "{}，{}{}",
                                    game.label(),
                                    description,
                                    if selected { "，当前游戏" } else { "" },
                                ))
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .py_3()
                                .px_1()
                                .child(div().text_size(px(16.)).child(game.label()))
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(Palette::of(cx).muted)
                                        .child(description),
                                )
                        })
                        .checked(selected)
                        .on_click(move |_, window, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                if !this.busy() && this.state.profile.game != game {
                                    this.navigate(Intent::Game(game), window, cx);
                                }
                            });
                        }),
                    );
                }
                menu
            })
            .into_any_element()
    }

    #[cfg(not(target_os = "macos"))]
    fn nav_button(
        &self,
        page: Page,
        n: usize,
        active: bool,
        expansion: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = Palette::of(cx);
        let selected = self.page == page;
        let variant = motion::selection(
            ("navigation-motion", n),
            selected,
            self.nav_hover == Some(page),
            active,
            window,
            cx,
        );
        div()
            .id(("navigation-hover", n))
            .relative()
            .w_full()
            .child(
                Button::new(("navigation", n))
                    .custom(variant)
                    .selected(selected)
                    .accessibility_label(page.title())
                    .tooltip(page.title())
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(Icon::new(page.icon()).size(px(16.)).flex_shrink_0())
                            .when(expansion > 0.001, |el| {
                                el.child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .opacity(expansion)
                                        .child(page.title()),
                                )
                            }),
                    )
                    .w_full()
                    .justify_start()
                    .small()
                    .h(px(NAV_HEIGHT))
                    .px_2()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if window.viewport_size().width < px(1080.) {
                            this.nav_expanded = false;
                        }
                        this.navigate(Intent::Page(page), window, cx);
                    })),
            )
            // Managed tooltips install their own hover listener on the button.
            // Keep navigation motion on this wrapper to avoid duplicate listeners.
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.nav_hover = Some(page);
                } else if this.nav_hover == Some(page) {
                    this.nav_hover = None;
                }
                cx.notify();
            }))
            .when(selected && !MAC, |el| {
                el.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(10.))
                        .w(px(3.))
                        .h(px(18.))
                        .rounded(px(2.))
                        .bg(p.accent),
                )
            })
            .into_any_element()
    }
}
impl Render for Toolkit {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::of(cx);
        let open =
            !matches!(self.overlay, crate::overlay::OverlayState::None) && !self.drawer_closing;
        let presence = base::Presence::new("inspector-presence", open)
            .transition(motion::transition(if open {
                motion::DRAWER_ENTER
            } else {
                motion::DRAWER_EXIT
            }))
            .sample(window, cx);
        if !presence.should_render() && self.drawer_closing {
            self.overlay = crate::overlay::OverlayState::None;
            self.drawer_closing = false;
        }
        if open && !self.drawer_was_open {
            self.drawer_return_focus = window.focused(cx);
            // Table arrow-key navigation retains focus while related details update.
            if matches!(self.overlay, crate::overlay::OverlayState::Rules) {
                self.drawer_focus.focus(window, cx);
            }
        } else if !open && self.drawer_was_open && !window.has_active_dialog(cx) {
            self.drawer_return_focus
                .as_ref()
                .unwrap_or(&self.focus)
                .focus(window, cx);
        }
        self.drawer_was_open = open;
        let scene = motion::Scene::new(self.page, self.state.profile.game, self.patch_changes);
        if self.last_scene != Some(scene) {
            self.last_scene = Some(scene);
            self.scene_generation = self.scene_generation.wrapping_add(1);
        }
        let settings = self.page == Page::Settings;
        let leading = if self.settings_only {
            heading("设置").into_any_element()
        } else {
            self.platform_leading(window, cx)
        };
        let tools = if self.settings_only {
            window_chrome::drag_space("settings-toolbar-drag")
                .w_full()
                .into_any_element()
        } else {
            self.platform_tools(window, cx)
        };
        let tools = div()
            .flex()
            .items_center()
            .w_full()
            .h_full()
            .min_w_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .h_full()
                    .flex_1()
                    .min_w_0()
                    .child(tools),
            )
            .child(self.update_toolbar(window, cx));
        let title_bar = window_chrome::title_bar(leading, tools, window, p);
        let nav_width = self.navigation_width(window);
        let nav_expansion = base::transition(
            "navigation-expansion",
            if self.nav_expanded { 1.0_f32 } else { 0.0 },
            motion::transition(motion::DRAWER_ENTER),
            window,
            cx,
        );
        let docked =
            window.viewport_size().width - px(nav_width) >= px(560. + self.inspector_width);
        let show_page = !open || docked;
        let mut work = div().flex_1().min_w_0().min_h_0().flex().flex_col();
        if let Some(error) = self.local_error.as_ref().or(self.state.error.as_ref())
            && !self.shows_update_error(error, UpdateTarget::Application)
            && !(settings && self.shows_update_error(error, UpdateTarget::Rules))
        {
            work = work.child(
                div()
                    .px_4()
                    .py_2()
                    .flex_shrink_0()
                    .bg(p.danger_bg)
                    .text_color(p.danger)
                    .child(error.clone())
                    .with_animation(
                        "workspace-error-enter",
                        Animation::new(motion::PAGE).with_easing(motion::ease_out),
                        |el, t| el.opacity(t),
                    ),
            );
        }
        for (n, task) in self
            .state
            .pending_deployments
            .iter()
            .filter(|t| !settings && t.game == self.state.profile.game)
            .enumerate()
        {
            let id = task.installation_id.clone();
            work = work.child(
                div()
                    .px_4()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        muted(
                            task.blocked
                                .clone()
                                .unwrap_or_else(|| "等待游戏退出后复核并应用".into()),
                            p,
                        )
                        .flex_1(),
                    )
                    .child(
                        Button::new(("cancel-pending", n))
                            .small()
                            .label("取消待应用")
                            .disabled(self.busy())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.dispatch(Command::CancelPending(id.clone()), cx)
                            })),
                    ),
            );
        }
        if !settings
            && self.state.operations.iter().any(|o| {
                o.game == self.state.profile.game
                    && o.state == poe2_core::OperationState::RecoveryRequired
            })
            && self.page != Page::History
        {
            work = work.child(
                div()
                    .px_4()
                    .py_2()
                    .bg(p.danger_bg)
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_color(p.danger)
                            .child("有操作需要恢复，请先检查记录。"),
                    )
                    .child(
                        Button::new("recovery-history")
                            .small()
                            .label("查看记录")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.navigate(Intent::Page(Page::History), window, cx)
                            })),
                    ),
            );
        }
        let mut content = div().relative().flex().flex_1().min_h_0().min_w_0();
        if show_page {
            let page = match self.page {
                Page::Patch => self.patch(window, cx),
                Page::Market => self.market(cx),
                Page::History => self.history(cx),
                Page::Settings => self.settings(cx),
            };
            let surface = div()
                .id(("page", self.page as usize))
                .flex_1()
                .min_w_0()
                .min_h_0()
                .h_full()
                .relative()
                .when(matches!(self.page, Page::Settings | Page::History), |el| {
                    el.overflow_y_scroll().p_4()
                })
                .child(page)
                .with_animation(
                    ("page-enter", self.scene_generation),
                    Animation::new(motion::PAGE).with_easing(motion::ease_out),
                    |el, t| el.opacity(t).top(px(6. * (1. - t))),
                );
            content = content.child(surface);
        }
        if presence.should_render() {
            if docked {
                content = content.child(
                    div()
                        .id("inspector-resizer")
                        .w(px(4. * presence.progress))
                        .h_full()
                        .flex_shrink_0()
                        .cursor(CursorStyle::ResizeLeftRight)
                        .hover(|el| el.bg(p.accent.opacity(0.4)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.resizing_inspector = true;
                                cx.stop_propagation();
                            }),
                        ),
                );
            }
            if let Some(inspector) = self.inspector(presence.progress, docked, window, cx) {
                content = content.child(inspector);
            }
        }
        work = work.child(content);
        if !settings
            && (self.busy() || self.state.refreshing.is_some())
            && !self
                .update_activity(UpdateTarget::Application)
                .phase
                .running()
        {
            work = work.child(
                div()
                    .h(px(32.))
                    .flex_shrink_0()
                    .px_4()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(Spinner::new().with_size(px(13.)).color(p.accent))
                    .child(
                        muted(self.state.status.clone(), p)
                            .flex_1()
                            .min_w_0()
                            .truncate(),
                    )
                    .when(self.cancellable || self.state.refreshing.is_some(), |el| {
                        el.child(
                            Button::new("cancel-task")
                                .ghost()
                                .small()
                                .label("取消任务")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.services.cancel();
                                    cx.notify();
                                })),
                        )
                    }),
            );
        }
        let overlay_nav = !MAC
            && nav_expansion > 0.001
            && window.viewport_size().width < px(1080.)
            && !self.settings_only;
        let mut body = div().relative().flex().flex_1().min_h_0().w_full();
        if overlay_nav {
            body = body.child(div().w(px(nav_width)).flex_shrink_0());
        }
        if !overlay_nav && let Some(nav) = self.platform_navigation(nav_expansion, window, cx) {
            body = body.child(nav);
        }
        body = body.child(work);
        if overlay_nav && let Some(nav) = self.platform_navigation(nav_expansion, window, cx) {
            body = body.child(nav);
        }
        div()
            .key_context("Toolkit")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(if self.native_backdrop && !surface_is_solid(cx) {
                p.sidebar.opacity(0.32)
            } else {
                p.sidebar
            })
            .text_color(p.text)
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.services.note_user_activity();
                if this.resizing_inspector {
                    if event.pressed_button == Some(MouseButton::Left) {
                        this.inspector_width =
                            f32::from(window.viewport_size().width - event.position.x)
                                .clamp(270., 420.);
                        cx.notify();
                    } else {
                        this.resizing_inspector = false;
                    }
                }
            }))
            .on_key_down(cx.listener(|this, _, _, _| this.services.note_user_activity()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.services.note_user_activity()),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.resizing_inspector = false;
                }),
            )
            .on_action(cx.listener(|this, _: &navigation::Patch, window, cx| {
                if !this.settings_only {
                    this.navigate(Intent::Page(Page::Patch), window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &navigation::Market, window, cx| {
                if !this.settings_only {
                    this.navigate(Intent::Page(Page::Market), window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &navigation::History, window, cx| {
                if !this.settings_only {
                    this.navigate(Intent::Page(Page::History), window, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &navigation::CheckForUpdates, window, cx| {
                    if window.has_active_dialog(cx) {
                        return;
                    }
                    if !this.settings_only {
                        if MAC {
                            this.open_settings(cx);
                        } else {
                            this.navigate(Intent::Page(Page::Settings), window, cx);
                        }
                    }
                    if !window.has_active_dialog(cx) {
                        this.check_application_update(cx);
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &navigation::Settings, window, cx| {
                if !this.settings_only {
                    this.navigate(Intent::Page(Page::Settings), window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &navigation::ClosePanel, window, cx| {
                this.navigate(Intent::Close, window, cx);
            }))
            .on_action(cx.listener(|this, _: &navigation::Refresh, window, cx| {
                if !this.settings_only
                    && !window.has_active_dialog(cx)
                    && !this.busy()
                    && this.state.profile.market.is_some()
                {
                    this.dispatch(Command::Refresh, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &navigation::Search, window, cx| {
                if !this.settings_only {
                    this.navigate(Intent::Page(Page::Market), window, cx);
                    if !window.has_active_dialog(cx) && this.page == Page::Market {
                        this.search.read(cx).focus_handle(cx).focus(window, cx);
                    }
                }
            }))
            .child(title_bar)
            .child(body)
            .children(Root::render_dialog_layer(window, cx))
    }
}
