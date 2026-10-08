use super::*;
impl Toolkit {
    pub(super) fn platform_tools(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap_2()
            .flex_1()
            .h_full()
            .min_w_0()
            .children(Page::MAIN.into_iter().enumerate().map(|(n, page)| {
                Button::new(("mac-navigation", n))
                    .custom(motion::selection(
                        ("mac-navigation-motion", n),
                        self.page == page,
                        self.nav_hover == Some(page),
                        window.is_window_active(),
                        window,
                        cx,
                    ))
                    .small()
                    .label(page.title())
                    .selected(self.page == page)
                    .accessibility_label(page.title())
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        this.nav_hover = if *hovered { Some(page) } else { None };
                        cx.notify();
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.navigate(Intent::Page(page), window, cx)
                    }))
            }))
            .child(
                window_chrome::drag_space("mac-toolbar-drag")
                    .flex_1()
                    .min_w(px(12.)),
            )
            .child(self.workspace_toolbar(window, cx))
            .into_any_element()
    }
    pub(super) fn platform_leading(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.game_switcher(window, cx)
    }
    pub(crate) fn navigation_width(&self, _: &Window) -> f32 {
        0.
    }
    pub(super) fn platform_navigation(
        &self,
        _: f32,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<AnyElement> {
        None
    }
}
