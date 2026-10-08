//! Window-scoped selectors. A market is committed only after a league is chosen.
use crate::{app::Toolkit, components::*, overlay::Intent, theme::*};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    component::{
        Disableable, Icon, IconName, Selectable, Sizable, WindowExt,
        button::{Button, ButtonVariants},
        menu::{DropdownMenu, PopupMenuItem},
        popover::Popover,
    },
    *,
};
use poe2_core::{MarketScope, Realm};
use poe2_service::Command;

impl Toolkit {
    pub(crate) fn workspace_toolbar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let compact = window.viewport_size().width < px(1100.);
        let icon_client = window.viewport_size().width < px(1000.)
            && self.application_presentation().toolbar_visible;
        let client_width = base::transition(
            "client-context-width",
            if icon_client {
                32_f32
            } else if compact {
                120.
            } else {
                156.
            },
            crate::motion::transition(crate::motion::PAGE),
            window,
            cx,
        );
        let blocked = self.busy() || window.has_active_dialog(cx);
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(self.market_selector(compact, blocked, cx))
            .child(self.client_selector(compact, icon_client, client_width, blocked, cx))
            .into_any_element()
    }

    pub(crate) fn refresh_market_button(&self, cx: &mut Context<Self>) -> AnyElement {
        Button::new("refresh-market")
            .ghost()
            .small()
            .h(px(CONTROL_HEIGHT))
            .icon(IconName::RotateCw)
            .label("刷新")
            .accessibility_label(if self.state.refreshing.is_some() {
                "正在刷新行情"
            } else {
                "刷新行情"
            })
            .tooltip("刷新当前市场行情")
            .loading(self.state.refreshing.is_some())
            .disabled(
                self.busy()
                    || self.state.refreshing.is_some()
                    || self.state.profile.market.is_none(),
            )
            .on_click(cx.listener(|this, _, _, cx| this.dispatch(Command::Refresh, cx)))
            .into_any_element()
    }

    fn market_selector(&self, compact: bool, blocked: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::of(cx);
        let scope = self.state.profile.market.as_ref();
        let name = scope
            .map(|s| {
                let league = self.state.leagues.iter().find(|l| {
                    l.game == s.game
                        && l.realm == s.realm
                        && l.id == s.league
                        && l.hardcore == s.hardcore
                });
                format!(
                    "{} · {}{}",
                    s.realm.label(),
                    league.map(|l| l.name.as_str()).unwrap_or(&s.league),
                    if s.hardcore { " · 硬核" } else { "" }
                )
            })
            .unwrap_or_else(|| "选择市场…".into());
        let display = if compact {
            scope
                .map(|s| {
                    self.state
                        .leagues
                        .iter()
                        .find(|l| {
                            l.game == s.game
                                && l.realm == s.realm
                                && l.id == s.league
                                && l.hardcore == s.hardcore
                        })
                        .map(|l| l.name.clone())
                        .unwrap_or_else(|| s.league.clone())
                })
                .unwrap_or_else(|| "市场".into())
        } else {
            name.clone()
        };
        let services = self.services.clone();
        let owner = cx.entity().downgrade();
        let open_owner = owner.clone();
        let draft = self.market_draft.clone();
        let open_draft = draft.clone();
        let game = self.state.profile.game;
        Popover::new("workspace-market-popover")
            .open(self.market_picker_open && !blocked)
            .on_open_change(move |open, _, cx| {
                let _ = open_owner.update(cx, |this, cx| {
                    this.market_picker_open = *open && !this.busy();
                    if *open {
                        *open_draft.borrow_mut() = this
                            .state
                            .profile
                            .market
                            .as_ref()
                            .map(|s| s.realm)
                            .or_else(|| {
                                this.state
                                    .profile
                                    .installation
                                    .as_ref()
                                    .and_then(|i| i.client_realm)
                            })
                            .unwrap_or_default();
                    }
                    cx.notify();
                });
            })
            .trigger(
                Button::new("workspace-market")
                    .ghost()
                    .small()
                    .h(px(CONTROL_HEIGHT))
                    .w(px(if compact { 156. } else { 204. }))
                    .disabled(blocked)
                    .accessibility_label(format!("行情市场：{name}"))
                    .tooltip(format!("行情市场：{name}"))
                    .child(
                        div()
                            .flex()
                            .w_full()
                            .items_center()
                            .gap_2()
                            .child(Icon::new(IconName::Globe).size(px(14.)).text_color(p.muted))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::NORMAL)
                                    .child(display),
                            )
                            .child(
                                Icon::new(IconName::ChevronDown)
                                    .size(px(12.))
                                    .text_color(p.muted),
                            ),
                    ),
            )
            .content(move |_, _, cx| {
                let state = services.snapshot();
                let p = Palette::of(cx);
                let realm = *draft.borrow();
                let mut body = div()
                    .id("market-picker-content")
                    .on_key_down(|event, window, cx| {
                        match event.keystroke.key.as_str() {
                            "down" | "right" => window.focus_next(cx),
                            "up" | "left" => window.focus_prev(cx),
                            _ => return,
                        }
                        cx.stop_propagation();
                    })
                    .w(px(340.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(heading("行情市场"))
                    .child(muted("选择完整赛季后生效；关闭面板保留当前市场。", p));
                let realms = div().flex().gap_2().children(
                    [Realm::International, Realm::China]
                        .into_iter()
                        .enumerate()
                        .map(|(n, candidate)| {
                            let draft = draft.clone();
                            Button::new(("market-realm", n))
                                .label(candidate.label())
                                .small()
                                .flex_1()
                                .selected(candidate == realm)
                                .disabled(state.busy)
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    *draft.borrow_mut() = candidate;
                                    cx.notify();
                                }))
                        }),
                );
                body = body.child(realms);
                let mut list = div()
                    .id("market-leagues")
                    .max_h(px(300.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_1();
                let mut any = false;
                for current in [true, false] {
                    let leagues = state
                        .leagues
                        .iter()
                        .filter(|l| l.game == game && l.realm == realm && l.current == current)
                        .collect::<Vec<_>>();
                    if !leagues.is_empty() {
                        list = list.child(
                            muted(
                                if current {
                                    "当前赛季"
                                } else {
                                    "其他赛季"
                                },
                                p,
                            )
                            .pt_2()
                            .px_2(),
                        );
                    }
                    for (n, league) in leagues.into_iter().enumerate() {
                        any = true;
                        let scope = MarketScope {
                            game: league.game,
                            realm: league.realm,
                            league: league.id.clone(),
                            hardcore: league.hardcore,
                        };
                        let checked = state.profile.market.as_ref() == Some(&scope);
                        let owner = owner.clone();
                        let label = format!(
                            "{}{}",
                            league.name,
                            if league.hardcore {
                                " · 硬核"
                            } else {
                                " · 普通"
                            }
                        );
                        list = list.child(
                            Button::new((
                                if current {
                                    "current-league"
                                } else {
                                    "other-league"
                                },
                                n,
                            ))
                            .ghost()
                            .w_full()
                            .justify_start()
                            .accessibility_label(label.clone())
                            .child(div().w_full().text_left().child(label))
                            .small()
                            .when(checked, |b| b.icon(IconName::Check))
                            .disabled(state.busy || state.profile.game != game)
                            .on_click(cx.listener(
                                move |popover, _, window, cx| {
                                    popover.dismiss(window, cx);
                                    let _ = owner.update(cx, |this, cx| {
                                        if this.busy() || this.state.profile.game != scope.game {
                                            return;
                                        }
                                        let mut profile = this.state.profile.clone();
                                        if profile.market.as_ref() == Some(&scope) {
                                            return;
                                        }
                                        profile.market = Some(scope.clone());
                                        this.navigate(Intent::Market(profile.into()), window, cx);
                                    });
                                },
                            )),
                        );
                    }
                }
                if !any {
                    list = list.child(
                        muted(
                            if state.busy {
                                "正在获取赛季…"
                            } else {
                                "暂无可用赛季，请重新获取。"
                            },
                            p,
                        )
                        .py_3(),
                    );
                }
                let reload = owner.clone();
                body.child(list)
                    .when(!state.league_warnings.is_empty(), |body| {
                        body.child(
                            Button::new("market-load-warning")
                                .ghost()
                                .small()
                                .w_full()
                                .h_auto()
                                .label("部分赛季未能获取，可重试 · 查看原因")
                                .tooltip(state.league_warnings.join("\n")),
                        )
                    })
                    .child(
                        Button::new("market-reload-leagues")
                            .ghost()
                            .small()
                            .icon(IconName::RotateCw)
                            .label("重新获取赛季列表")
                            .disabled(state.busy)
                            .on_click(move |_, _, cx| {
                                let _ = reload.update(cx, |this, cx| {
                                    this.dispatch(Command::DiscoverLeagues, cx)
                                });
                            }),
                    )
            })
            .into_any_element()
    }

    fn client_selector(
        &self,
        compact: bool,
        icon_only: bool,
        width: f32,
        blocked: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = Palette::of(cx);
        let label = self
            .state
            .profile
            .installation
            .as_ref()
            .map(|i| {
                format!(
                    "{} · {}",
                    i.client_realm.map(|r| r.label()).unwrap_or("服区待确认"),
                    if i.client_kind == poe2_core::ClientKind::Unknown {
                        "客户端"
                    } else {
                        i.client_kind.label()
                    }
                )
            })
            .unwrap_or_else(|| "选择客户端…".into());
        let display = if compact {
            self.state
                .profile
                .installation
                .as_ref()
                .map(|i| i.client_kind.label().to_owned())
                .unwrap_or_else(|| "客户端".into())
        } else {
            label.clone()
        };
        let description = self
            .state
            .profile
            .installation
            .as_ref()
            .map(|i| {
                format!(
                    "客户端：{label} · {}\n{}\n{}",
                    i.client_kind.label(),
                    i.root.display(),
                    if i.can_write {
                        "资源检查通过；应用前仍会复核"
                    } else {
                        "资源暂不可写；打开管理查看原因"
                    }
                )
            })
            .unwrap_or_else(|| "选择补丁的目标客户端；不影响独立浏览行情".into());
        let owner = cx.entity().downgrade();
        let services = self.services.clone();
        Button::new("workspace-installation")
            .ghost()
            .small()
            .h(px(CONTROL_HEIGHT))
            .w(px(width))
            .disabled(blocked)
            .accessibility_label(format!("目标客户端：{label}"))
            .tooltip(description)
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::FolderOpen)
                            .size(px(14.))
                            .text_color(p.muted),
                    )
                    .when(!icon_only, |el| {
                        el.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.))
                                .font_weight(FontWeight::NORMAL)
                                .child(display),
                        )
                        .child(
                            Icon::new(IconName::ChevronDown)
                                .size(px(12.))
                                .text_color(p.muted),
                        )
                    }),
            )
            .dropdown_menu(move |mut menu, _, _| {
                let state = services.snapshot();
                let game = state.profile.game;
                let selected = state.profile.installation.as_ref().map(|i| &i.root);
                let mut clients = std::collections::BTreeMap::new();
                for i in &state.installations {
                    if i.game == game {
                        clients.insert(
                            i.root.clone(),
                            (
                                format!(
                                    "{} · {}",
                                    i.client_kind.label(),
                                    i.client_realm.map(|r| r.label()).unwrap_or("服区待确认")
                                ),
                                "已保存安装",
                            ),
                        );
                    }
                }
                for c in &state.client_discovery.clients {
                    let i = &c.identity;
                    if i.game == game {
                        clients.insert(
                            i.root.clone(),
                            (
                                format!(
                                    "{} · {}",
                                    i.client_kind.label(),
                                    i.client_realm.map(|r| r.label()).unwrap_or("服区待确认")
                                ),
                                "已发现，选择后检查",
                            ),
                        );
                    }
                }
                if let Some(i) = &state.profile.installation {
                    clients.insert(
                        i.root.clone(),
                        (
                            format!(
                                "{} · {}",
                                i.client_kind.label(),
                                i.client_realm.map(|r| r.label()).unwrap_or("服区待确认")
                            ),
                            if i.can_write {
                                "资源已检查"
                            } else {
                                "资源暂不可写"
                            },
                        ),
                    );
                }
                menu = menu.min_w(px(340.)).max_w(px(430.));
                if clients.is_empty() {
                    menu = menu.item(PopupMenuItem::new("暂无已保存安装").disabled(true));
                }
                for (path, (name, status)) in clients {
                    let checked = selected == Some(&path);
                    let owner = owner.clone();
                    let text_path = path.display().to_string();
                    menu = menu.item(
                        PopupMenuItem::element(move |_, cx| {
                            div()
                                .id(SharedString::from(format!("client-menu-{text_path}")))
                                .role(Role::Group)
                                .aria_label(format!("{name} · {status} · {text_path}"))
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .py_2()
                                .child(format!("{name} · {status}"))
                                .child(
                                    muted(text_path.clone(), Palette::of(cx))
                                        .max_w(px(335.))
                                        .truncate(),
                                )
                        })
                        .checked(checked)
                        .disabled(state.busy)
                        .on_click(move |_, window, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                if !this.busy() && this.state.profile.game == game {
                                    this.navigate(Intent::Client(path.clone()), window, cx);
                                }
                            });
                        }),
                    );
                }
                let owner = owner.clone();
                menu.separator().item(
                    PopupMenuItem::new("管理客户端…")
                        .icon(IconName::FolderOpen)
                        .on_click(move |_, window, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                this.navigate(Intent::Installation, window, cx)
                            });
                        }),
                )
            })
            .into_any_element()
    }
}
