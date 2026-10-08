//! A transient titlebar status, never a permanent check button or launch panel.
use crate::{
    app::Toolkit,
    motion,
    theme::{CONTROL_HEIGHT, MAC, Palette},
};
use gpui_kit::{
    component::{
        Disableable, Sizable, WindowExt,
        button::{Button, ButtonVariants},
        progress::Progress,
    },
    prelude::FluentBuilder,
    *,
};
use poe2_service::{Command, update_activity::UpdateTarget};

impl Toolkit {
    pub(crate) fn check_application_update(&mut self, cx: &mut Context<Self>) {
        if self.state.application_update.configured
            && !self.busy()
            && !self.application_presentation().running
        {
            self.dispatch(Command::CheckApplication, cx);
        }
    }
    pub(crate) fn application_primary_action(&mut self, cx: &mut Context<Self>) {
        if self.busy() || self.application_presentation().running {
            return;
        }
        if self.application_presentation().can_install {
            let requester = cx.entity().downgrade();
            cx.defer(move |cx| crate::app::request_application_install(requester, cx));
        } else if self.application_presentation().needs_installer {
            cx.open_url(&poe2_service::app_updates::download_page());
        } else {
            self.check_application_update(cx);
        }
    }

    pub(crate) fn update_toolbar(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let view = self.application_presentation();
        let presence = base::Presence::new("application-title-presence", view.toolbar_visible)
            .transition(motion::transition(motion::PAGE))
            .sample(window, cx);
        if !presence.should_render() {
            return div().w_0().into_any_element();
        }
        let p = Palette::of(cx);
        let generation = self.update_activity(UpdateTarget::Application).generation as usize;
        let step = view.motion_step();
        let label = view.toolbar_label();
        let indicator = base::transition(
            "update-progress-presence",
            if view.running { 1_f32 } else { 0. },
            motion::transition(motion::CONTROL),
            window,
            cx,
        );
        let color = base::transition(
            "update-control-color",
            if view.ready || view.available {
                p.accent
            } else {
                p.muted
            },
            motion::transition(motion::CONTROL),
            window,
            cx,
        );
        let width = if MAC { 96. } else { 104. };
        div()
            .id("application-update-control")
            .relative()
            .flex_shrink_0()
            .w(px((width + 8.) * presence.progress))
            .opacity(presence.progress * if window.is_window_active() { 1. } else { 0.65 })
            .overflow_hidden()
            .child(
                Button::new("application-update-toolbar")
                    .ghost()
                    .small()
                    .w(px(width))
                    .h(px(CONTROL_HEIGHT))
                    .ml(px(8.))
                    .disabled(window.has_active_dialog(cx) || self.busy() || view.running)
                    .accessibility_label(format!("软件更新：{label}"))
                    .tooltip(format!("{}\n{}", view.title, view.detail))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(color)
                            .child(label)
                            .with_animation(
                                ("update-label", generation * 8 + step),
                                Animation::new(motion::CONTROL).with_easing(motion::ease_out),
                                |el, t| el.opacity(t).top(px(2. * (1. - t))),
                            ),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.application_primary_action(cx))),
            )
            .when(indicator > 0.001, |el| {
                el.child(
                    div()
                        .absolute()
                        .left(px(20.))
                        .right(px(12.))
                        .bottom(px(2.))
                        .opacity(indicator)
                        .child(
                            Progress::new(("application-title-progress", generation))
                                .accessibility_label("程序更新进度")
                                .loading(view.running && view.progress.is_none())
                                .value(view.progress.unwrap_or(100.))
                                .with_size(px(1.5))
                                .color(p.accent),
                        ),
                )
            })
            .into_any_element()
    }
}
