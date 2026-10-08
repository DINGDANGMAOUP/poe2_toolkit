//! One active work surface. Drafts keep their original revision until saved.
use crate::motion::Disclosure;
use crate::{app::Toolkit, components::*, navigation::Page, theme::*};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    component::{
        Disableable, IconName, WindowExt,
        button::{Button, ButtonVariants},
        dialog::DialogButtonProps,
    },
    *,
};
use poe2_core::{GameId, Operation, Profile};
use poe2_service::Command;

#[derive(Clone, Default)]
pub(crate) enum OverlayState {
    #[default]
    None,
    Quote,
    Rules,
    Change,
    History(Operation),
}
#[derive(Clone)]
pub(crate) enum Intent {
    Close,
    Page(Page),
    Game(GameId),
    Market(Box<Profile>),
    Edit,
    Installation,
    Client(std::path::PathBuf),
    MarketPicker,
    InstallApplication,
    HistoryDetail(Operation),
}
impl Toolkit {
    pub(crate) fn show_drawer(&mut self, overlay: OverlayState) {
        self.drawer_closing = false;
        self.overlay = overlay;
    }

    pub(crate) fn close_drawer(&mut self) {
        self.drawer_closing = !matches!(self.overlay, OverlayState::None);
    }

    pub(crate) fn navigate(&mut self, intent: Intent, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        if self.dirty() && !matches!(intent, Intent::Close | Intent::Edit) {
            // Keep the work surface behind its confirmation instead of closing
            // and reopening it when the user chooses to continue editing.
            let previous = self.overlay.clone();
            let owner = cx.entity().downgrade();
            window.open_alert_dialog(cx, move |dialog, _, _| {
                let accept = owner.clone();
                let cancel = owner.clone();
                let intent = intent.clone();
                let previous = previous.clone();
                dialog
                    .title("有尚未保存的方案")
                    .description("放弃修改后继续，或返回当前方案继续编辑。")
                    .close_button(false)
                    .button_props(
                        DialogButtonProps::default()
                            .show_cancel(true)
                            .ok_text("放弃并继续")
                            .cancel_text("继续编辑"),
                    )
                    .on_ok(move |_, window, cx| {
                        let _ = accept.update(cx, |this, cx| {
                            this.draft = None;
                            this.perform_intent(intent.clone(), window, cx)
                        });
                        true
                    })
                    .on_cancel(move |_, _, cx| {
                        let _ = cancel.update(cx, |this, cx| {
                            this.show_drawer(previous.clone());
                            cx.notify();
                        });
                        true
                    })
            });
        } else {
            self.perform_intent(intent, window, cx);
        }
        cx.notify();
    }
    fn perform_intent(&mut self, intent: Intent, window: &mut Window, cx: &mut Context<Self>) {
        self.close_drawer();
        match intent {
            Intent::Close => self.focus.focus(window, cx),
            Intent::Page(page) => {
                if page == Page::Settings && !self.settings_only {
                    self.open_settings(cx);
                } else {
                    self.page = page;
                }
                self.focus.focus(window, cx);
            }
            Intent::Game(game) => {
                if !self.busy() && game != self.state.profile.game {
                    self.dispatch(Command::SwitchGame(game), cx);
                }
            }
            Intent::Market(profile) => {
                if !self.busy() {
                    self.dispatch(Command::SaveProfile(profile), cx);
                }
            }
            Intent::Edit => {
                self.page = Page::Patch;
                self.patch_changes = false;
                self.show_drawer(OverlayState::Rules);
                self.drawer_focus.focus(window, cx);
            }
            Intent::HistoryDetail(operation) => {
                self.show_drawer(OverlayState::History(operation));
                self.drawer_focus.focus(window, cx);
            }
            Intent::InstallApplication => {
                if !self.busy() && self.application_presentation().can_install {
                    self.dispatch(Command::InstallApplication, cx);
                }
            }
            Intent::Installation => self.installation_dialog(window, cx),
            Intent::Client(path) => {
                if !self.busy() {
                    self.dispatch(Command::ConnectDetected(path), cx);
                }
            }
            Intent::MarketPicker => {
                *self.market_draft.borrow_mut() = self
                    .state
                    .profile
                    .market
                    .as_ref()
                    .map(|s| s.realm)
                    .or_else(|| {
                        self.state
                            .profile
                            .installation
                            .as_ref()
                            .and_then(|i| i.client_realm)
                    })
                    .unwrap_or_default();
                self.market_picker_open = true;
            }
        }
        cx.notify();
    }
    pub(crate) fn confirm_close(
        &mut self,
        quit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::component::WindowExt;
        if window.has_active_dialog(cx) {
            return;
        }
        let owner = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let owner = owner.clone();
            dialog
                .title("关闭前放弃未保存的方案？")
                .description("继续编辑可保留当前修改。")
                .close_button(false)
                .button_props(
                    DialogButtonProps::default()
                        .show_cancel(true)
                        .ok_text("放弃并关闭")
                        .cancel_text("继续编辑"),
                )
                .on_ok(move |_, window, cx| {
                    let _ = owner.update(cx, |this, _| {
                        this.draft = None;
                    });
                    if quit {
                        cx.quit();
                    } else {
                        window.remove_window();
                    }
                    true
                })
        });
    }
    pub(crate) fn confirm_command(
        &mut self,
        title: &str,
        description: String,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous = self.overlay.clone();
        let owner = cx.entity().downgrade();
        let title = title.to_owned();
        let action = match &command {
            Command::SaveProfile(profile) if profile.auto_apply => "允许并保存",
            Command::Apply(_) => "确认应用",
            Command::Restore(_) => "校验并还原",
            _ => "确认",
        };
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let accept = owner.clone();
            let cancel = owner.clone();
            let command = command.clone();
            let previous = previous.clone();
            dialog
                .title(title.clone())
                .description(description.clone())
                .close_button(false)
                .button_props(
                    DialogButtonProps::default()
                        .show_cancel(true)
                        .ok_text(action)
                        .cancel_text("返回"),
                )
                .on_ok(move |_, _, cx| {
                    let _ = accept.update(cx, |this, cx| {
                        if matches!(command, Command::SaveProfile(_)) {
                            this.saving_draft = true;
                        } else {
                            this.close_drawer();
                        }
                        this.dispatch(command.clone(), cx);
                    });
                    true
                })
                .on_cancel(move |_, _, cx| {
                    let _ = cancel.update(cx, |this, cx| {
                        this.show_drawer(previous.clone());
                        cx.notify();
                    });
                    true
                })
        });
    }
    fn installation_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::input::{Input, InputState};
        let owner = cx.entity().downgrade();
        let services = self.services.clone();
        let expanded = std::rc::Rc::new(std::cell::RefCell::new(std::collections::BTreeSet::new()));
        let manual_open = std::rc::Rc::new(std::cell::Cell::new(false));
        let game = self.state.profile.game;
        let manual = cx
            .new(|cx| InputState::new(window, cx).placeholder("粘贴游戏目录、程序或资源文件路径"));
        if !self.busy() {
            self.dispatch(Command::DiscoverInstallations, cx);
        }
        window.open_dialog(cx, move |dialog, _, cx| {
            // open_dialog invokes this builder while Toolkit is being updated.
            // Read the shared service snapshot instead of reborrowing its owner.
            let state = services.snapshot();
            let busy = state.busy;
            let p = Palette::of(cx);
            let clients = state.client_discovery.clients.iter().collect::<Vec<_>>();
            let scan = owner.clone();
            let browse = owner.clone();
            let file = owner.clone();
            let use_path = owner.clone();
            let path_input = manual.clone();
            let manual_visible = manual_open.get();
            let toggle_manual = manual_open.clone();
            let manual_owner = owner.clone();
            let empty = if busy {
                "正在检查客户端，请稍候…"
            } else if state.client_discovery.scanned {
                "未发现客户端，可在下方手动选择。"
            } else {
                "点击自动识别，或手动选择客户端。"
            };
            let mut body = div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(muted("自动查找安装记录与游戏库；选择后检查资源结构。", p))
                        .child(
                            Button::new("discover-clients")
                                .label("自动识别")
                                .icon(IconName::Search)
                                .disabled(busy)
                                .on_click(move |_, _, cx| {
                                    let _ = scan.update(cx, |this, cx| {
                                        this.dispatch(Command::DiscoverInstallations, cx)
                                    });
                                }),
                        ),
                )
                .child(
                    div()
                        .id("installation-list")
                        .max_h(px(230.))
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .when(clients.is_empty(), |list| {
                            list.child(muted(empty, p).py_3())
                        })
                        .children(clients.iter().enumerate().map(|(n, candidate)| {
                            let choose = owner.clone();
                            let client = &candidate.identity;
                            let path = client.root.clone();
                            let evidence_path = path.clone();
                            let evidence_is_open = expanded.borrow().contains(&path);
                            let expanded = expanded.clone();
                            let refresh = owner.clone();
                            let current = state
                                .profile
                                .installation
                                .as_ref()
                                .is_some_and(|i| i.root == path);
                            let storage = match client.storage {
                                poe2_core::StorageKind::Ggpk => "GGPK",
                                poe2_core::StorageKind::Bundles => "Bundles2",
                                poe2_core::StorageKind::Unknown => "未知资源",
                            };
                            div()
                                .id(("client-card", n))
                                .role(Role::Group)
                                .aria_label(format!(
                                    "{} · {} · {}",
                                    client.game.label(),
                                    client
                                        .client_realm
                                        .map(|r| r.label())
                                        .unwrap_or("服区待确认"),
                                    client.client_kind.label()
                                ))
                                .flex()
                                .items_center()
                                .gap_3()
                                .p_3()
                                .rounded(px(8.))
                                .border_1()
                                .border_color(p.border)
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(format!(
                                            "{} · {} · {}",
                                            client.game.label(),
                                            client
                                                .client_realm
                                                .map(|r| r.label())
                                                .unwrap_or("服区待确认"),
                                            client.client_kind.label()
                                        ))
                                        .child(muted(path.display().to_string(), p))
                                        .child(muted(
                                            format!("{} · 来源：{}", storage, candidate.source),
                                            p,
                                        ))
                                        .child(
                                            Disclosure::new(("client-evidence-motion", n))
                                                .open(evidence_is_open)
                                                .child(
                                                    Button::new(("client-evidence", n))
                                                        .ghost()
                                                        .label(if evidence_is_open {
                                                            "收起识别依据"
                                                        } else {
                                                            "识别依据"
                                                        })
                                                        .icon(if evidence_is_open {
                                                            IconName::ChevronUp
                                                        } else {
                                                            IconName::ChevronDown
                                                        })
                                                        .on_click(move |_, _, cx| {
                                                            if evidence_is_open {
                                                                expanded
                                                                    .borrow_mut()
                                                                    .remove(&evidence_path);
                                                            } else {
                                                                expanded
                                                                    .borrow_mut()
                                                                    .insert(evidence_path.clone());
                                                            }
                                                            let _ = refresh
                                                                .update(cx, |_, cx| cx.notify());
                                                        }),
                                                )
                                                .content(
                                                    div()
                                                        .id(("client-evidence-detail", n))
                                                        .flex()
                                                        .flex_col()
                                                        .gap_1()
                                                        .role(Role::Group)
                                                        .aria_label(
                                                            client.client_evidence.join("；"),
                                                        )
                                                        .children(
                                                            client
                                                                .client_evidence
                                                                .iter()
                                                                .map(|e| muted(e.clone(), p)),
                                                        ),
                                                ),
                                        ),
                                )
                                .child(
                                    Button::new(("connect-client", n))
                                        .label(if current {
                                            "重新检查"
                                        } else if client.game != game {
                                            "切换游戏并连接"
                                        } else {
                                            "选择"
                                        })
                                        .disabled(busy)
                                        .on_click(move |_, _, cx| {
                                            let _ = choose.update(cx, |this, cx| {
                                                this.dispatch(
                                                    Command::ConnectDetected(path.clone()),
                                                    cx,
                                                )
                                            });
                                        }),
                                )
                        })),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new("browse-installation")
                                .label("浏览文件夹…")
                                .icon(IconName::FolderOpen)
                                .disabled(busy)
                                .on_click(move |_, _, cx| {
                                    let _ = browse
                                        .update(cx, |this, cx| this.select_client_path(false, cx));
                                }),
                        )
                        .child(
                            Button::new("browse-client-file")
                                .label("选择程序或资源文件…")
                                .disabled(busy)
                                .on_click(move |_, _, cx| {
                                    let _ = file
                                        .update(cx, |this, cx| this.select_client_path(true, cx));
                                }),
                        ),
                )
                .child(
                    Disclosure::new("manual-client-path")
                        .open(manual_visible)
                        .child(
                            Button::new("toggle-manual-path")
                                .ghost()
                                .label(if manual_visible {
                                    "收起手动路径"
                                } else {
                                    "手动输入路径…"
                                })
                                .on_click(move |_, _, cx| {
                                    toggle_manual.set(!manual_visible);
                                    let _ = manual_owner.update(cx, |_, cx| cx.notify());
                                }),
                        )
                        .content(
                            div()
                                .w_full()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div().flex_1().min_w_0().child(
                                        Input::new(&manual)
                                            .disabled(!manual_visible || busy)
                                            .w_full(),
                                    ),
                                )
                                .child(
                                    Button::new("use-client-path")
                                        .label("使用此路径")
                                        .disabled(busy || !manual_visible)
                                        .on_click(move |_, _, cx| {
                                            let text = path_input.read(cx).value().to_string();
                                            let path = text.trim().trim_matches('"');
                                            if !path.is_empty() {
                                                let _ = use_path.update(cx, |this, cx| {
                                                    this.dispatch(
                                                        Command::ConnectDetected(path.into()),
                                                        cx,
                                                    )
                                                });
                                            }
                                        }),
                                ),
                        ),
                )
                .child(muted(
                    "选择安装不会更改正在浏览的市场。价格补丁需匹配目标客户端服区。",
                    p,
                ));
            if let Some(installation) = &state.profile.installation {
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(format!(
                            "当前已连接：{} · {} · {}",
                            game.label(),
                            installation
                                .client_realm
                                .map(|r| r.label())
                                .unwrap_or("服区待确认"),
                            installation.client_kind.label()
                        ))
                        .child(muted(installation.root.display().to_string(), p))
                        .child(muted(
                            if installation.can_write {
                                "可生成补丁预览；应用前仍会复核。"
                            } else {
                                "当前资源暂不可写。"
                            },
                            p,
                        ))
                        .children(
                            installation
                                .reasons
                                .iter()
                                .take(3)
                                .map(|reason| muted(reason.clone(), p)),
                        ),
                );
            }
            if let Some(error) = &state.error {
                body = body.child(div().text_color(p.danger).child(error.clone()));
            }
            if !state.client_discovery.warnings.is_empty() {
                body = body.child(
                    div()
                        .id("discovery-warnings")
                        .max_h(px(90.))
                        .overflow_y_scroll()
                        .children(
                            state
                                .client_discovery
                                .warnings
                                .iter()
                                .map(|warning| muted(warning.clone(), p)),
                        ),
                );
            }
            dialog
                .title(format!("管理 {} 客户端", game.label()))
                .w(px(660.))
                .child(body)
        });
    }
    pub(crate) fn adjacent_quote(&mut self, direction: isize, cx: &mut Context<Self>) {
        let delegate = self.table.read(cx).delegate();
        let Some(snapshot) = &delegate.snapshot else {
            return;
        };
        let index = self
            .selected
            .as_ref()
            .and_then(|q| {
                delegate
                    .rows
                    .iter()
                    .position(|n| snapshot.quotes[*n].item_id == q.item_id)
            })
            .unwrap_or(0);
        let next = index
            .saturating_add_signed(direction)
            .min(delegate.rows.len().saturating_sub(1));
        self.selected = delegate.rows.get(next).map(|n| snapshot.quotes[*n].clone());
        if self.selected.is_some() {
            self.table
                .update(cx, |table, cx| table.set_selected_row(next, cx));
        }
        cx.notify();
    }
}
