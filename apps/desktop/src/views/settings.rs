use crate::{app::Toolkit, components::*, theme::*};
use gpui_kit::{
    component::{
        Disableable, Icon, IconName, Side, Sizable,
        button::Button,
        menu::{DropdownMenu, PopupMenuItem},
        switch::Switch,
    },
    prelude::FluentBuilder,
    *,
};
use poe2_service::{Command, update_activity::UpdateTarget};
impl Toolkit {
    pub(crate) fn settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::of(cx);
        let application = self.update_activity(UpdateTarget::Application);
        let rules = self.update_activity(UpdateTarget::Rules);
        let application_view = self.application_presentation();
        let appearance = cx.global::<AppearanceSettings>().appearance();
        let owner = cx.entity().downgrade();
        let appearance_picker = Button::new("appearance-picker")
            .small()
            .h(px(CONTROL_HEIGHT))
            .w(px(132.))
            .accessibility_label(format!("外观：{}", appearance.label()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .w_full()
                    .justify_between()
                    .child(appearance.label())
                    .child(Icon::new(IconName::ChevronDown).size(px(14.))),
            )
            .dropdown_menu(move |mut menu, _, _| {
                menu = menu.min_w(px(160.)).check_side(Side::Right);
                for mode in Appearance::ALL {
                    let owner = owner.clone();
                    menu = menu.item(
                        PopupMenuItem::new(mode.label())
                            .checked(mode == appearance)
                            .on_click(move |_, window, cx| {
                                let _ = owner.update(cx, |this, cx| {
                                    match set_appearance(mode, window, cx) {
                                        Ok(()) => {
                                            this.native_backdrop =
                                                crate::window_chrome::initialize(window, cx);
                                            this.local_error = None;
                                        }
                                        Err(error) => {
                                            this.local_error =
                                                Some(format!("无法保存外观：{error}"))
                                        }
                                    }
                                    cx.notify();
                                });
                            }),
                    );
                }
                menu
            });
        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(group(
                "通用",
                div()
                    .flex()
                    .flex_col()
                    .child(row(
                        "关闭窗口后继续运行",
                        "通过系统托盘继续运行、暂停更新或退出。",
                        Switch::new("close-to-tray")
                            .checked(self.state.close_to_tray)
                            .accessibility_label("关闭窗口后继续运行")
                            .disabled(self.busy())
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                this.dispatch(Command::SetCloseToTray(*checked), cx);
                            })),
                        p,
                    ))
                    .child(separator(p))
                    .child(row(
                        "外观",
                        "选择浅色、深色或跟随系统。",
                        appearance_picker,
                        p,
                    ))
                    .child(separator(p))
                    .child(row(
                        "减少动态效果",
                        if cx.global::<crate::motion::MotionSettings>().system_reduced {
                            "已跟随系统减少动态效果。"
                        } else {
                            "减少切换动画和循环动效。"
                        },
                        Switch::new("reduce-motion")
                            .accessibility_label("减少动态效果")
                            .checked(
                                cx.global::<crate::motion::MotionSettings>()
                                    .reduced_by_user(),
                            )
                            .on_change(cx.listener(|this, reduced: &bool, _, cx| {
                                if let Err(error) = crate::motion::set_reduced(*reduced, cx) {
                                    this.local_error =
                                        Some(format!("无法保存动态效果设置：{error}"));
                                }
                                cx.notify();
                            })),
                        p,
                    )),
                p,
            ))
            .child(group(
                "版本与更新",
                div()
                    .flex()
                    .flex_col()
                    .child(row(
                        format!("PoE Toolkit {}", env!("CARGO_PKG_VERSION")),
                        application_view.caption.clone(),
                        div().flex().items_center().gap_2().child(
                            Button::new("check-application")
                                .label(application_view.settings_label())
                                .loading(application.phase.running())
                                .when(application.phase.running(), |button| {
                                    button.icon(IconName::Loader)
                                })
                                .min_w(px(108.))
                                .small()
                                .h(px(CONTROL_HEIGHT))
                                .disabled(
                                    (self.busy() && !application.phase.running())
                                        || (!application_view.configured
                                            && !application_view.ready),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.application_primary_action(cx);
                                })),
                        ),
                        p,
                    ))
                    .child(
                        muted(
                            "启动时自动下载更新，空闲时检查新版本。下载后由你选择重启。",
                            p,
                        )
                        .px_4()
                        .pb_3(),
                    )
                    .when(self.state.update_configured, |el| {
                        el.child(separator(p))
                            .child(row(
                                "数据规则",
                                format!("当前规则：{}", self.state.rule_version),
                                Button::new("check-rules")
                                    .label(crate::update_feedback::button_label(&rules.phase))
                                    .loading(rules.phase.running())
                                    .when(rules.phase.running(), |button| {
                                        button.icon(IconName::Loader)
                                    })
                                    .min_w(px(108.))
                                    .small()
                                    .h(px(CONTROL_HEIGHT))
                                    .disabled(
                                        (self.busy() && !rules.phase.running())
                                            || !self.state.update_configured,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.dispatch(Command::CheckRules, cx)
                                    })),
                                p,
                            ))
                            .child(self.rule_update_feedback(cx))
                    }),
                p,
            ))
            .child(group(
                "本地数据",
                div().flex().flex_col().child(row(
                    "数据目录",
                    self.state.data_dir.display().to_string(),
                    Button::new("reveal-data")
                        .label("打开")
                        .small()
                        .h(px(CONTROL_HEIGHT))
                        .on_click(
                            cx.listener(|this, _, _, cx| cx.reveal_path(&this.state.data_dir)),
                        ),
                    p,
                )),
                p,
            ))
            .into_any_element()
    }
}
