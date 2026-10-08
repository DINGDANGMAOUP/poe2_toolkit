//! Related details participate in layout; they never own the editing session.
use crate::{
    app::Toolkit, components::*, draft::DraftSession, overlay::OverlayState, presentation::*,
    theme::*,
};
use gpui_kit::{
    component::{
        Disableable, IconName, Sizable,
        button::{Button, ButtonVariants},
        switch::Switch,
    },
    prelude::FluentBuilder,
    *,
};
use poe2_core::Profile;
use poe2_service::Command;
impl Toolkit {
    pub(crate) fn dirty(&self) -> bool {
        self.draft.as_ref().is_some_and(DraftSession::dirty)
    }
    pub(crate) fn edit_profile(&mut self) -> &mut Profile {
        &mut self
            .draft
            .get_or_insert_with(|| DraftSession::new(&self.state.profile))
            .profile
    }
    pub(crate) fn editing_profile(&self) -> &Profile {
        self.draft
            .as_ref()
            .map(|d| &d.profile)
            .unwrap_or(&self.state.profile)
    }
    pub(crate) fn review_plan(&self) -> Option<&std::sync::Arc<poe2_core::PatchPlan>> {
        self.review_plans.get(&self.state.profile.game)
    }
    pub(crate) fn review_is_current(&self) -> bool {
        self.review_plan().is_some_and(|plan| {
            self.state
                .plan
                .as_ref()
                .is_some_and(|active| active.id == plan.id)
                && crate::workbench::preview_matches(
                    plan,
                    &self.state.profile,
                    &self.state.rule_version,
                    chrono::Utc::now(),
                )
        })
    }
    pub(crate) fn save_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = &self.draft else { return };
        if !draft.dirty() || !draft.current(&self.state.profile) || self.busy() {
            return;
        }
        let command = Command::SaveProfile(Box::new(draft.profile.clone()));
        if draft.profile.auto_apply && !draft.original.auto_apply {
            let profile = &draft.profile;
            self.confirm_command("允许当前方案自动应用？", format!("{} · {}\n目标：{}\n应用运行期间刷新后复核资源，游戏退出后才能写入。可在方案规则中关闭。", profile.game.label(), profile.market.as_ref().map(|s| format!("{} · {}",s.realm.label(),s.league)).unwrap_or_else(|| "无行情依赖".into()), profile.installation.as_ref().map(|i| i.root.display().to_string()).unwrap_or_default()), command, window, cx);
        } else {
            self.dispatch(command, cx);
            self.saving_draft = self.local_error.is_none();
        }
    }
    fn rules_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::of(cx);
        let profile = self.editing_profile();
        let changed = self.draft.as_ref().is_some_and(|d| {
            d.original.features != d.profile.features
                || d.original.annotation != d.profile.annotation
        });
        let mut body = div().flex().flex_col().gap_3();
        for (n, label, detail, checked) in [
            (
                0,
                "国际参考价",
                "国服缺价时允许来源明确的国际传奇参考",
                profile.annotation.allow_international_reference,
            ),
            (
                1,
                "显示报价口径",
                "在价格后附上挂牌或交易所等口径",
                profile.annotation.detailed,
            ),
            (
                2,
                "自动刷新",
                "应用运行期间定时获取行情",
                profile.auto_refresh,
            ),
            (
                3,
                "自动应用",
                "需先手动成功应用当前方案；游戏退出后写入",
                profile.auto_apply,
            ),
        ] {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(label)
                            .child(
                                Switch::new(("policy", n as usize))
                                    .accessibility_label(label)
                                    .checked(checked)
                                    .disabled(
                                        self.busy() || self.drawer_closing || (n == 3 && changed),
                                    )
                                    .on_change(cx.listener(move |this, value: &bool, _, cx| {
                                        let draft = this.edit_profile();
                                        if n < 2 {
                                            draft.auto_apply = false;
                                        }
                                        match n {
                                            0 => {
                                                draft.annotation.allow_international_reference =
                                                    *value
                                            }
                                            1 => draft.annotation.detailed = *value,
                                            2 => {
                                                draft.auto_refresh = *value;
                                                if !*value {
                                                    draft.auto_apply = false;
                                                }
                                            }
                                            _ => {
                                                draft.auto_apply = *value;
                                                if *value {
                                                    draft.auto_refresh = true;
                                                }
                                            }
                                        }
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(muted(detail, p)),
            );
        }
        body = body
            .child(separator(p))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(muted(
                        format!("缺价保留 {} 分钟", profile.annotation.retention_minutes),
                        p,
                    ))
                    .child(
                        Button::new("retention")
                            .small()
                            .label("调整")
                            .disabled(self.busy() || self.drawer_closing)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let draft = this.edit_profile();
                                draft.auto_apply = false;
                                draft.annotation.retention_minutes =
                                    match draft.annotation.retention_minutes {
                                        0 => 60,
                                        60 => 240,
                                        _ => 0,
                                    };
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(muted(format!("每 {} 分钟刷新", profile.refresh_minutes), p))
                    .child(
                        Button::new("interval")
                            .small()
                            .label("调整")
                            .disabled(self.busy() || self.drawer_closing)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let draft = this.edit_profile();
                                draft.refresh_minutes = match draft.refresh_minutes {
                                    15 => 60,
                                    60 => 120,
                                    _ => 15,
                                };
                                cx.notify();
                            })),
                    ),
            );
        if changed {
            body = body.child(muted(
                "功能或显示规则已变更，自动应用已关闭。保存并手动应用后才能重新开启。",
                p,
            ));
        }
        body.into_any_element()
    }
    pub(crate) fn inspector(
        &self,
        progress: f32,
        docked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let p = Palette::of(cx);
        let (title, body, footer, _wide) = match &self.overlay {
            OverlayState::None => return None,
            OverlayState::Rules => (
                "方案规则",
                self.rules_content(cx),
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("inspector-cancel-draft")
                            .small()
                            .ghost()
                            .label("取消修改")
                            .disabled(!self.dirty() || self.busy() || self.drawer_closing)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.draft = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("inspector-save-draft")
                            .small()
                            .primary()
                            .label("保存方案")
                            .disabled(
                                !self.dirty()
                                    || self.busy()
                                    || self.drawer_closing
                                    || self
                                        .draft
                                        .as_ref()
                                        .is_some_and(|d| !d.current(&self.state.profile)),
                            )
                            .on_click(
                                cx.listener(|this, _, window, cx| this.save_draft(window, cx)),
                            ),
                    )
                    .into_any_element(),
                false,
            ),
            OverlayState::Change => {
                let change = self.review_plan()?.changes.get(self.selected_change?)?;
                let before = change.before.clone();
                let after = change.after.clone();
                let mut body = div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(heading(format!(
                        "{} · {}",
                        change.feature.label(),
                        change.record_id
                    )))
                    .child(muted(change.resource.clone(), p))
                    .child(muted("写入前", p))
                    .child(change.before.clone())
                    .child(muted("写入后", p))
                    .child(div().text_color(p.success).child(change.after.clone()))
                    .child(separator(p))
                    .child(muted(change.reason.clone(), p));
                if let Some(e) = &change.price_evidence {
                    body = body.child(muted(
                        format!(
                            "行情快照：{}\n获取：{}\n有效至：{}\n报价身份：{}",
                            e.snapshot_id,
                            timestamp(e.fetched_at),
                            timestamp(e.expires_at),
                            e.quote_ids.join("、")
                        ),
                        p,
                    ));
                }
                let footer = div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("copy-before")
                            .small()
                            .label("复制原文")
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(before.clone()))
                            }),
                    )
                    .child(
                        Button::new("copy-after")
                            .small()
                            .label("复制结果")
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(after.clone()))
                            }),
                    );
                (
                    "变化详情",
                    body.into_any_element(),
                    footer.into_any_element(),
                    false,
                )
            }
            OverlayState::Quote => {
                let q = self.selected.as_ref()?;
                let body = div()
                    .id("quote-details")
                    .role(Role::Group)
                    .aria_label(format!(
                        "报价详情：{}，{}，{}，{}",
                        q.name,
                        q.display_price(),
                        q.statistic.label(),
                        q.provider
                    ))
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(heading(q.name.clone()))
                    .child(div().text_size(px(26.)).child(q.display_price()))
                    .child(muted(
                        q.market_label(&self.state.snapshot.as_ref()?.scope),
                        p,
                    ))
                    .child(row(
                        "报价口径",
                        q.statistic.label(),
                        muted(q.provider.clone(), p),
                        p,
                    ))
                    .child(row(
                        "来源时间",
                        q.observed_at
                            .map(timestamp)
                            .unwrap_or_else(|| "未提供".into()),
                        muted(format!("获取于 {}", timestamp(q.fetched_at)), p),
                        p,
                    ))
                    .child(muted(
                        if q.variant.is_empty() {
                            "无已知变体条件".into()
                        } else {
                            q.variant.label()
                        },
                        p,
                    ))
                    .child(muted(
                        if q.permits_annotation() {
                            "符合名称标注基础条件；仍需唯一资源匹配"
                        } else {
                            "仅行情参考 · 不用于无条件名称标注"
                        },
                        p,
                    ))
                    .children(q.reasons.iter().map(|r| muted(r.clone(), p)));
                let footer = div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("quote-previous")
                            .disabled(self.drawer_closing)
                            .label("上一项")
                            .on_click(cx.listener(|this, _, _, cx| this.adjacent_quote(-1, cx))),
                    )
                    .child(
                        Button::new("quote-next")
                            .disabled(self.drawer_closing)
                            .label("下一项")
                            .on_click(cx.listener(|this, _, _, cx| this.adjacent_quote(1, cx))),
                    );
                (
                    "报价详情",
                    body.into_any_element(),
                    footer.into_any_element(),
                    false,
                )
            }
            OverlayState::History(op) => {
                let id = op.id.clone();
                let body = div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(heading(op.summary.clone()))
                    .child(muted(state_label(&op.state), p))
                    .child(muted(format!("时间：{}", timestamp(op.started_at)), p))
                    .child(muted(format!("安装：{}", op.installation_id), p))
                    .child(muted(format!("计划：{}", op.plan_id), p))
                    .child(muted(
                        format!(
                            "计划行情快照：{}",
                            op.snapshot_id.as_deref().unwrap_or("无价格依赖")
                        ),
                        p,
                    ))
                    .child(muted(
                        op.scope
                            .as_ref()
                            .map(|s| format!("来源市场：{} · {}", s.realm.label(), s.league))
                            .unwrap_or_else(|| "无市场依赖".into()),
                        p,
                    ));
                let enabled = !matches!(
                    op.state,
                    poe2_core::OperationState::RolledBack
                        | poe2_core::OperationState::FailedUnchanged
                );
                let desc = format!(
                    "{} · 操作 {}\n将校验并还原该次操作的资源文件。",
                    op.game.label(),
                    id
                );
                let footer = Button::new("restore-operation")
                    .label("校验并还原…")
                    .disabled(self.busy() || self.drawer_closing || !enabled)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.confirm_command(
                            "还原操作",
                            desc.clone(),
                            Command::Restore(id.clone()),
                            window,
                            cx,
                        )
                    }));
                (
                    "操作详情",
                    body.into_any_element(),
                    footer.into_any_element(),
                    false,
                )
            }
        };
        // Clip a full-width reading surface instead of reflowing text on every
        // frame. On narrow windows it overlays the returning page while exiting.
        let panel = div()
            .id("workspace-inspector")
            .relative()
            .left(px(16. * (1. - progress)))
            .h_full()
            .min_h_0()
            .flex_shrink_0()
            .when(docked, |el| {
                el.w(px(self.inspector_width))
                    .border_l_1()
                    .border_color(p.border)
            })
            .when(!docked, |el| el.w_full().min_w_0())
            .overflow_hidden()
            .opacity(progress)
            .bg(p.canvas)
            .flex()
            .flex_col()
            .track_focus(&self.drawer_focus)
            .when(self.drawer_closing, |el| {
                el.capture_any_mouse_down(|_, _, cx| cx.stop_propagation())
                    .capture_any_mouse_up(|_, _, cx| cx.stop_propagation())
                    .capture_key_down(|_, _, cx| cx.stop_propagation())
            })
            .child(
                div()
                    .px_4()
                    .py_2()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(heading(title))
                    .child(
                        Button::new("close-inspector")
                            .ghost()
                            .small()
                            .icon(if docked {
                                IconName::Close
                            } else {
                                IconName::ChevronLeft
                            })
                            .disabled(self.drawer_closing)
                            .accessibility_label(if docked {
                                "关闭详情"
                            } else {
                                "返回列表"
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.navigate(crate::overlay::Intent::Close, window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .id("inspector-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_4()
                    .child(body),
            )
            .child(div().px_4().py_3().flex_shrink_0().child(footer));
        let _ = window;
        Some(
            div()
                .id("inspector-clip")
                .h_full()
                .min_h_0()
                .flex_shrink_0()
                .overflow_hidden()
                .when(docked, |el| el.w(px(self.inspector_width * progress)))
                .when(!docked, |el| {
                    el.absolute()
                        .top_0()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .occlude()
                })
                .child(panel)
                .into_any_element(),
        )
    }
}
