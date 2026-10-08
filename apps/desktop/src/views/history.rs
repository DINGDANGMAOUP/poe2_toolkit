use crate::overlay::Intent;
use crate::{app::Toolkit, components::*, presentation::*, theme::*};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    component::{Icon, IconName, Sizable, button::Button},
    *,
};
use poe2_core::OperationState;
impl Toolkit {
    pub(crate) fn history(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::of(cx);
        if !self
            .state
            .operations
            .iter()
            .any(|op| op.game == self.state.profile.game)
        {
            return empty(
                IconName::Undo2,
                "还没有操作记录",
                "应用补丁后，可在这里查看结果并还原修改。",
                p,
            )
            .into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(muted(
                "每次修改独立保存备份。还原前会校验文件，避免覆盖外部修改。",
                p,
            ))
            .child(
                div().bg(p.surface).overflow_hidden().children(
                    self.state
                        .operations
                        .iter()
                        .filter(|op| op.game == self.state.profile.game)
                        .enumerate()
                        .map(|(n, op)| {
                            let operation = op.clone();
                            let can_restore = matches!(
                                op.state,
                                OperationState::Committed
                                    | OperationState::RecoveryRequired
                                    | OperationState::Prepared
                                    | OperationState::Preparing
                                    | OperationState::Writing
                                    | OperationState::Verifying
                            );
                            let needs_recovery =
                                can_restore && op.state != OperationState::Committed;
                            div()
                                .when(n > 0, |el| el.child(separator(p)))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_4()
                                        .px_3()
                                        .py_2()
                                        .child(
                                            Icon::new(if needs_recovery {
                                                IconName::TriangleAlert
                                            } else {
                                                IconName::CircleCheck
                                            })
                                            .size(px(20.))
                                            .text_color(if needs_recovery {
                                                p.danger
                                            } else {
                                                p.muted
                                            }),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .flex()
                                                .flex_col()
                                                .gap_1()
                                                .child(op.summary.clone())
                                                .child(muted(
                                                    format!(
                                                        "{} · {}",
                                                        timestamp(op.started_at),
                                                        state_label(&op.state)
                                                    ),
                                                    p,
                                                )),
                                        )
                                        .child(
                                            Button::new(("restore", n))
                                                .label("查看详情")
                                                .small()
                                                .h(px(CONTROL_HEIGHT))
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.navigate(
                                                            Intent::HistoryDetail(
                                                                operation.clone(),
                                                            ),
                                                            window,
                                                            cx,
                                                        );
                                                        this.drawer_focus.focus(window, cx);
                                                        cx.notify();
                                                    },
                                                )),
                                        ),
                                )
                                .into_any_element()
                        }),
                ),
            )
            .into_any_element()
    }
}
