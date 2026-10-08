use crate::theme::Palette;
use gpui_kit::{
    component::{Icon, IconName},
    *,
};

pub fn muted(text: impl Into<SharedString>, p: Palette) -> Div {
    div()
        .text_size(px(12.))
        .text_color(p.muted)
        .child(text.into())
}
pub fn heading(text: impl Into<SharedString>) -> Div {
    div().font_weight(FontWeight::SEMIBOLD).child(text.into())
}
pub fn group(title: impl Into<SharedString>, content: impl IntoElement, p: Palette) -> Div {
    div().flex().flex_col().gap_2().child(heading(title)).child(
        div()
            .bg(p.surface)
            .border_1()
            .border_color(p.border)
            .rounded(px(8.))
            .overflow_hidden()
            .child(content),
    )
}
pub fn row(
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    control: impl IntoElement,
    p: Palette,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap_4()
        .px_4()
        .py_3()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(title.into())
                .child(muted(detail, p)),
        )
        .child(div().flex_shrink_0().child(control))
}
pub fn separator(p: Palette) -> Div {
    div().h(px(1.)).mx_4().bg(p.border)
}
pub fn empty(icon: IconName, title: &str, detail: &str, p: Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_3()
        .py_10()
        .px_6()
        .child(Icon::new(icon).size(px(32.)).text_color(p.muted))
        .child(heading(title.to_owned()))
        .child(muted(detail.to_owned(), p))
}
