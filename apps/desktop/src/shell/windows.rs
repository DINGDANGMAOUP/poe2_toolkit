use super::*;
impl Toolkit {
    pub(super) fn platform_tools(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .h_full()
            .flex_1()
            .min_w_0()
            .child(
                window_chrome::drag_space("windows-toolbar-drag")
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
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                Button::new("toggle-navigation")
                    .ghost()
                    .small()
                    .icon(IconName::Menu)
                    .accessibility_label("展开或收起导航")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.nav_expanded = !this.nav_expanded;
                        cx.notify();
                    })),
            )
            .child(self.game_switcher(window, cx))
            .into_any_element()
    }
    pub(crate) fn navigation_width(&self, window: &Window) -> f32 {
        if self.settings_only {
            0.
        } else if self.nav_expanded && window.viewport_size().width >= px(1080.) {
            window_chrome::NAV_EXPANDED
        } else {
            window_chrome::NAV_COMPACT
        }
    }
    pub(super) fn platform_navigation(
        &self,
        expansion: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.settings_only {
            return None;
        }
        let overlay = expansion > 0.001 && window.viewport_size().width < px(1080.);
        let active = window.is_window_active();
        let p = Palette::of(cx);
        let nav = div()
            .id("windows-navigation")
            .h_full()
            .flex_shrink_0()
            .w(px(window_chrome::NAV_COMPACT
                + (window_chrome::NAV_EXPANDED
                    - window_chrome::NAV_COMPACT)
                    * expansion))
            .overflow_hidden()
            .px_1()
            .py_2()
            .flex()
            .flex_col()
            .gap_2()
            .when(overlay, |el| {
                el.absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .occlude()
                    .bg(p.sidebar)
                    .shadow_lg()
            })
            .children(
                Page::MAIN
                    .into_iter()
                    .enumerate()
                    .map(|(n, page)| self.nav_button(page, n, active, expansion, window, cx)),
            )
            .child(div().flex_1())
            .child(self.nav_button(Page::Settings, 3, active, expansion, window, cx));
        Some(nav.into_any_element())
    }
}
