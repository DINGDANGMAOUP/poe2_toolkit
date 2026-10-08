use crate::{app::Toolkit, components::*, theme::*, views::market_table::CategoryFilter};
use gpui_kit::{
    component::{
        Icon, IconName, Selectable, Sizable,
        button::{Button, ButtonVariants},
        input::Input,
        menu::{DropdownMenu, PopupMenuItem},
        table::DataTable,
    },
    *,
};
impl Toolkit {
    pub(crate) fn market(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::of(cx);
        let owner = cx.entity().downgrade();
        let game = self.state.profile.game;
        let selected = self.category;
        let toolbar = div()
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .h(px(44.))
            .flex_shrink_0()
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.search)
                        .prefix(Icon::new(IconName::Search).size(px(15.)))
                        .small()
                        .h(px(CONTROL_HEIGHT)),
                ),
            )
            .child(self.refresh_market_button(cx))
            .child(
                Button::new("market-category")
                    .ghost()
                    .small()
                    .label(self.category.label())
                    .icon(IconName::ChevronDown)
                    .dropdown_menu(move |mut menu, _, _| {
                        for category in CategoryFilter::for_game(game).iter().copied() {
                            let owner = owner.clone();
                            menu = menu.item(
                                PopupMenuItem::new(category.label())
                                    .checked(selected == category)
                                    .on_click(move |_, _, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.category = category;
                                            this.filter_table(cx);
                                            cx.notify();
                                        });
                                    }),
                            );
                        }
                        menu
                    }),
            )
            .child(
                Button::new("market-quality")
                    .ghost()
                    .small()
                    .label("需留意")
                    .selected(self.quality_only)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.quality_only = !this.quality_only;
                        this.filter_table(cx);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("market-clear-filter")
                    .ghost()
                    .small()
                    .icon(IconName::Close)
                    .accessibility_label("清除搜索和筛选")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.category = CategoryFilter::All;
                        this.quality_only = false;
                        this.search
                            .update(cx, |search, cx| search.set_value("", window, cx));
                        this.filter_table(cx);
                        cx.notify();
                    })),
            );
        let mut body = div().size_full().flex().flex_col().child(toolbar).child(
            div().flex_1().min_h_0().bg(p.surface).child(
                DataTable::new(&self.table).stripe(MAC).with_size(if MAC {
                    component::Size::Small
                } else {
                    component::Size::Medium
                }),
            ),
        );
        let text = if let Some(snapshot) = &self.state.snapshot {
            let fresh = snapshot
                .quotes
                .iter()
                .filter(|q| q.quality_at(chrono::Utc::now()) == poe2_core::Quality::Fresh)
                .count();
            format!(
                "{} 项 · 获取于 {} · 有效 {} · 需留意 {}",
                self.table.read(cx).delegate().rows.len(),
                crate::presentation::timestamp(snapshot.fetched_at),
                fresh,
                snapshot.quotes.len() - fresh
            )
        } else {
            "尚无行情 · 通过顶部选择赛季后刷新".into()
        };
        body = body.child(
            div()
                .px_4()
                .py_2()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_2()
                .child(muted(text, p).flex_1().min_w_0().truncate())
                .child(
                    Button::new("market-warnings-toggle")
                        .ghost()
                        .small()
                        .label("来源说明")
                        .selected(self.show_market_warnings)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_market_warnings = !this.show_market_warnings;
                            cx.notify();
                        })),
                ),
        );
        body = body.child(
            crate::motion::Disclosure::new("market-source-disclosure")
                .open(self.show_market_warnings)
                .content(
                    div()
                        .id("market-source-warnings")
                        .max_h(px(110.))
                        .overflow_y_scroll()
                        .px_4()
                        .pb_2()
                        .child(muted(
                            "价格按原币种分别排序；选择条目查看口径、变体与证据。",
                            p,
                        ))
                        .children(
                            self.state
                                .snapshot
                                .iter()
                                .flat_map(|s| s.warnings.iter())
                                .map(|w| muted(w.clone(), p)),
                        ),
                ),
        );
        body.into_any_element()
    }
}
