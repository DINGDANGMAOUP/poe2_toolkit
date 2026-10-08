use crate::{
    app::Toolkit,
    components::*,
    navigation::Page,
    overlay::{Intent, OverlayState},
    theme::*,
    workbench::{self, NextStep},
};
use gpui_kit::{
    component::{
        Disableable, IconName, Selectable, Sizable,
        button::{Button, ButtonVariants},
        menu::{DropdownMenu, PopupMenuItem},
        switch::Switch,
        table::DataTable,
    },
    prelude::FluentBuilder,
    *,
};
use poe2_core::{Feature, PlanIntent};
use poe2_service::Command;
impl Toolkit {
    pub(crate) fn patch(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::of(cx);
        let status = workbench::status(&self.state, chrono::Utc::now());
        let profile = self.editing_profile();
        let writable = self
            .state
            .profile
            .installation
            .as_ref()
            .is_some_and(|i| i.can_write);
        let plan = self.review_plan();
        let stale = self.patch_changes && plan.is_some() && !self.review_is_current();
        let applicable = self.patch_changes
            && self.review_is_current()
            && plan.is_some_and(|p| p.blockers.is_empty() && !p.mutations.is_empty());
        let owner = cx.entity().downgrade();
        let mut tools = div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(44.))
            .px_4()
            .flex_shrink_0()
            .children(
                [(false, "方案"), (true, "变更")]
                    .into_iter()
                    .enumerate()
                    .map(|(n, (changes, label))| {
                        Button::new(("patch-tab", n))
                            .custom(crate::motion::selection(
                                ("patch-tab-motion", n),
                                self.patch_changes == changes,
                                self.patch_hover == Some(changes),
                                window.is_window_active(),
                                window,
                                cx,
                            ))
                            .small()
                            .label(label)
                            .selected(self.patch_changes == changes)
                            .disabled(changes && self.dirty())
                            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                this.patch_hover = if *hovered { Some(changes) } else { None };
                                cx.notify();
                            }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.patch_changes = changes;
                                this.close_drawer();
                                cx.notify();
                            }))
                    }),
            )
            .child(muted(profile.name.clone(), p).flex_1().min_w_0().truncate());
        if self.dirty() {
            tools = tools
                .child(
                    Button::new("cancel-draft")
                        .small()
                        .label("取消修改")
                        .disabled(self.busy())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.draft = None;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("save-draft")
                        .small()
                        .primary()
                        .label("保存方案")
                        .loading(self.saving_draft && self.busy())
                        .disabled(
                            self.busy()
                                || !self
                                    .draft
                                    .as_ref()
                                    .is_some_and(|d| d.current(&self.state.profile)),
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.save_draft(window, cx))),
                );
        } else {
            tools = tools
                .child(
                    Button::new("patch-rules")
                        .ghost()
                        .small()
                        .label("规则")
                        .selected(matches!(self.overlay, OverlayState::Rules))
                        .on_click(cx.listener(|this, _, window, cx| {
                            if matches!(this.overlay, OverlayState::Rules) && !this.drawer_closing {
                                this.close_drawer();
                                cx.notify();
                            } else {
                                this.navigate(Intent::Edit, window, cx)
                            }
                        })),
                )
                .child(
                    Button::new("patch-more")
                        .ghost()
                        .small()
                        .label("更多")
                        .disabled(self.busy() || !writable)
                        .dropdown_menu(move |menu, _, _| {
                            let owner = owner.clone();
                            menu.item(PopupMenuItem::new("预览清除当前标注").on_click(
                                move |_, _, cx| {
                                    let _ = owner.update(cx, |this, cx| {
                                        this.dispatch(Command::Preview { restore: true }, cx)
                                    });
                                },
                            ))
                        }),
                );
            if applicable {
                tools = tools.child(
                    Button::new("apply-preview")
                        .primary()
                        .small()
                        .label("应用补丁…")
                        .disabled(self.busy())
                        .on_click(cx.listener(|this, _, window, cx| this.apply_review(window, cx))),
                );
            } else {
                let action = if self.patch_changes && writable {
                    NextStep::Preview
                } else {
                    status.action
                };
                let action = if action == NextStep::Review {
                    NextStep::Preview
                } else {
                    action
                };
                let label = if action == NextStep::Preview {
                    "生成预览"
                } else {
                    status.label
                };
                tools = tools.child(
                    Button::new("patch-primary")
                        .primary()
                        .small()
                        .label(label)
                        .disabled(
                            self.busy()
                                || action == NextStep::None
                                || (action == NextStep::Refresh && self.state.refreshing.is_some()),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| match action {
                            NextStep::Client => this.navigate(Intent::Installation, window, cx),
                            NextStep::Market => this.navigate(Intent::MarketPicker, window, cx),
                            NextStep::Refresh => this.dispatch(Command::Refresh, cx),
                            NextStep::History => {
                                this.navigate(Intent::Page(Page::History), window, cx)
                            }
                            NextStep::Edit => this.navigate(Intent::Edit, window, cx),
                            NextStep::Preview | NextStep::Review => {
                                this.dispatch(Command::Preview { restore: false }, cx)
                            }
                            NextStep::None => {}
                        })),
                );
            }
        }
        let _preparation = status.preparation;
        let status_text = if self.dirty() {
            "方案有未保存修改 · 保存后才能生成预览".into()
        } else if stale {
            "此预览已失效或已执行，只供查阅；重新生成后才能应用。".into()
        } else {
            format!("{} · {}", status.title, status.detail)
        };
        let mut body = div()
            .size_full()
            .flex()
            .flex_col()
            .child(tools)
            .child(muted(status_text, p).px_4().py_2().flex_shrink_0());
        if self
            .draft
            .as_ref()
            .is_some_and(|d| !d.current(&self.state.profile))
        {
            body = body.child(
                div()
                    .px_4()
                    .py_2()
                    .bg(p.danger_bg)
                    .text_color(p.danger)
                    .child("方案版本已改变。草稿仍保留，请取消修改后重新核对。"),
            );
        }
        if self.patch_changes {
            if let Some(plan) = plan {
                for blocker in &plan.blockers {
                    body = body.child(
                        div()
                            .px_4()
                            .py_2()
                            .bg(p.danger_bg)
                            .text_color(p.danger)
                            .child(blocker.clone()),
                    );
                }
                body = body.child(
                    div()
                        .px_4()
                        .pb_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            muted(
                                format!(
                                    "{} 项变化 · {} 个文件 · {}",
                                    plan.changes.len(),
                                    plan.mutations.len(),
                                    crate::presentation::timestamp(plan.created_at)
                                ),
                                p,
                            )
                            .flex_1(),
                        )
                        .when(!plan.warnings.is_empty(), |el| {
                            el.child(
                                Button::new("patch-warnings")
                                    .ghost()
                                    .small()
                                    .label(format!("来源及适配说明 · {}", plan.warnings.len()))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_patch_warnings = !this.show_patch_warnings;
                                        cx.notify();
                                    })),
                            )
                        }),
                );
                body = body.child(
                    crate::motion::Disclosure::new("patch-warning-disclosure")
                        .open(self.show_patch_warnings)
                        .content(
                            div()
                                .id("patch-warning-scroll")
                                .max_h(px(110.))
                                .overflow_y_scroll()
                                .px_4()
                                .pb_2()
                                .children(plan.warnings.iter().map(|w| muted(w.clone(), p))),
                        ),
                );
                if plan.mutations.is_empty() && plan.blockers.is_empty() {
                    body = body.child(muted("本次检查无需写入。", p).px_4().pb_2());
                }
            }
            body = body.child(div().flex_1().min_h_0().child(
                DataTable::new(&self.changes).stripe(MAC).with_size(if MAC {
                    component::Size::Small
                } else {
                    component::Size::Medium
                }),
            ));
        } else {
            let mut list = div()
                .id("patch-scheme-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px_4()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .py_2()
                        .border_b_1()
                        .border_color(p.border)
                        .child(muted("补丁功能", p).flex_1())
                        .child(muted("启用", p)),
                );
            for (n, feature) in Feature::for_game(profile.game).iter().copied().enumerate() {
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .py_2()
                        .border_b_1()
                        .border_color(p.border)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap_3()
                                .child(div().w(px(104.)).child(feature.label()))
                                .child(muted(
                                    if feature.annotatable() {
                                        if feature == Feature::MapHints {
                                            "补充资源提示，不依赖行情"
                                        } else {
                                            "按物品身份与报价条件标注"
                                        }
                                    } else {
                                        "复杂变体仅供行情浏览"
                                    },
                                    p,
                                )),
                        )
                        .child(
                            Switch::new(("draft-feature", n))
                                .accessibility_label(feature.label())
                                .checked(profile.features.contains(&feature))
                                .disabled(self.busy() || !feature.annotatable())
                                .on_change(cx.listener(move |this, checked: &bool, _, cx| {
                                    let draft = this.edit_profile();
                                    draft.auto_apply = false;
                                    draft.features.retain(|f| *f != feature);
                                    if *checked {
                                        draft.features.push(feature);
                                    }
                                    cx.notify();
                                })),
                        ),
                );
            }
            list = list.child(
                muted(
                    format!(
                        "国际参考：{} · 缺价最多保留 {} 分钟 · 自动应用：{}",
                        if profile.annotation.allow_international_reference {
                            "允许"
                        } else {
                            "关闭"
                        },
                        profile.annotation.retention_minutes,
                        if profile.auto_apply {
                            "开启"
                        } else {
                            "关闭"
                        }
                    ),
                    p,
                )
                .py_3(),
            );
            if self.state.profile.installation.is_none() {
                list = list.child(
                    Button::new("discover-client")
                        .ghost()
                        .small()
                        .icon(IconName::FolderOpen)
                        .label("发现并连接客户端…")
                        .disabled(self.busy())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.navigate(Intent::Installation, window, cx)
                        })),
                );
            }
            body = body.child(list);
        }
        body.into_any_element()
    }
    fn apply_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() || self.dirty() || !self.review_is_current() {
            return;
        }
        let Some(plan) = self.review_plan() else {
            return;
        };
        if !plan.blockers.is_empty() || plan.mutations.is_empty() {
            return;
        }
        let clear = matches!(plan.intent, PlanIntent::ClearAnnotations);
        let description = format!(
            "{} · {}\n目标：{}\n{} 项变化 · {} 个文件\n{}",
            plan.game.label(),
            plan.scope
                .as_ref()
                .map(|s| format!("{} · {}", s.realm.label(), s.league))
                .unwrap_or_else(|| "无行情依赖".into()),
            plan.installation_root.display(),
            plan.changes.len(),
            plan.mutations.len(),
            if clear {
                "清除本应用拥有的标注，并关闭当前方案自动应用。"
            } else {
                "将复核目标、行情有效期和资源指纹；游戏运行时等待退出。"
            }
        );
        self.confirm_command(
            if clear {
                "清除当前标注"
            } else {
                "应用已审阅的变化"
            },
            description,
            Command::Apply(plan.id.clone()),
            window,
            cx,
        );
    }
}
