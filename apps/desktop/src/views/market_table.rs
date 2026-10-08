use crate::{components::*, presentation::*, theme::*};
use gpui_kit::component::{
    Icon, IconName, Sizable,
    button::{Button, ButtonVariants},
    table::{Column, ColumnSort, TableDelegate, TableState},
};
use gpui_kit::*;
use poe2_core::{MarketSnapshot, Quote};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CategoryFilter {
    All,
    Currency,
    Unique,
    TabletNames,
    TabletAffixes,
    Cards,
    Materials,
    Gems,
    Maps,
}
impl CategoryFilter {
    pub const ALL: [Self; 5] = [
        Self::All,
        Self::Currency,
        Self::Unique,
        Self::TabletNames,
        Self::TabletAffixes,
    ];
    pub fn for_game(game: poe2_core::GameId) -> &'static [Self] {
        if game == poe2_core::GameId::Poe1 {
            &[
                Self::All,
                Self::Currency,
                Self::Cards,
                Self::Materials,
                Self::Unique,
                Self::Gems,
                Self::Maps,
            ]
        } else {
            &Self::ALL
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Cards => "命运卡",
            Self::Materials => "材料",
            Self::Gems => "技能石",
            Self::Maps => "地图",
            Self::All => "全部物品",
            Self::Currency => "通货与材料",
            Self::Unique => "传奇装备",
            Self::TabletNames => "碑牌名称",
            Self::TabletAffixes => "碑牌词缀",
        }
    }
    pub fn matches(self, q: &Quote) -> bool {
        match self {
            Self::Cards => q.feature() == poe2_core::Feature::DivinationCards,
            Self::Materials => q.feature() == poe2_core::Feature::Materials,
            Self::Gems => q.feature() == poe2_core::Feature::Gems,
            Self::Maps => q.feature() == poe2_core::Feature::Maps,
            Self::All => true,
            Self::Currency => q.feature() == poe2_core::Feature::Currency,
            Self::Unique => q.feature() == poe2_core::Feature::Unique,
            Self::TabletNames => q.feature() == poe2_core::Feature::TabletNames,
            Self::TabletAffixes => q.feature() == poe2_core::Feature::TabletAffixes,
        }
    }
}

pub(crate) struct PriceTable {
    pub(crate) snapshot: Option<Arc<MarketSnapshot>>,
    pub(crate) rows: Vec<usize>,
    pub(crate) sort: Option<(usize, ColumnSort)>,
    pub(crate) selected_id: Option<String>,
    pub(crate) restoring_selection: bool,
}
impl TableDelegate for PriceTable {
    fn columns_count(&self, _: &App) -> usize {
        5
    }
    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }
    fn column(&self, n: usize, _: &App) -> Column {
        let (key, name, width) = [
            ("name", "物品", 280.),
            ("price", "参考价格", 140.),
            ("category", "分类", 135.),
            ("source", "市场 / 来源", 175.),
            ("quality", "数据状态", 110.),
        ][n];
        Column::new(key, name).width(px(width))
    }
    fn render_th(
        &mut self,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let name = self.column(column, cx).name;
        let direction = self
            .sort
            .filter(|(selected, _)| *selected == column)
            .map(|(_, direction)| direction)
            .unwrap_or_default();
        let (icon, status, next) = match direction {
            ColumnSort::Default => (IconName::ChevronsUpDown, "默认顺序", ColumnSort::Descending),
            ColumnSort::Descending => (IconName::SortDescending, "降序", ColumnSort::Ascending),
            ColumnSort::Ascending => (IconName::SortAscending, "升序", ColumnSort::Default),
        };
        // The library's sort glyph is mouse-only. Use one named, focusable
        // header button for the same action without adding a separate toolbar.
        Button::new(("sort-column", column))
            .ghost()
            .small()
            .w_full()
            .px_0()
            .accessibility_label(format!("按{name}排序，当前{status}"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .child(name)
                    .child(Icon::new(icon).size(px(12.))),
            )
            .on_click(cx.listener(move |table, _, window, cx| {
                cx.stop_propagation();
                table.delegate_mut().perform_sort(column, next, window, cx);
            }))
    }
    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let (title, detail) = if self.snapshot.is_some() {
            ("没有匹配的物品", "尝试其他关键词或分类。")
        } else {
            ("尚无行情数据", "请先选择赛季，然后刷新行情。")
        };
        empty(
            gpui_kit::component::IconName::Search,
            title,
            detail,
            Palette::of(cx),
        )
    }
    fn perform_sort(
        &mut self,
        column: usize,
        sort: ColumnSort,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.sort = Some((column, sort));
        self.sort_rows();
        let selected = self.selected_id.as_ref().and_then(|id| {
            self.snapshot.as_ref().and_then(|snapshot| {
                self.rows
                    .iter()
                    .position(|row| &snapshot.quotes[*row].item_id == id)
            })
        });
        let table = cx.entity();
        cx.defer(move |cx| {
            table.update(cx, |table, cx| {
                if let Some(row) = selected {
                    table.delegate_mut().restoring_selection = true;
                    table.set_selected_row(row, cx);
                }
                cx.notify();
            })
        });
    }
    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let q = &self.snapshot.as_ref().unwrap().quotes[self.rows[row]];
        let text = match column {
            0 => q.name.clone(),
            1 => q.display_price(),
            2 => category_label(&q.category).to_owned(),
            3 => format!(
                "{} · {}",
                if q.is_reference_for(&self.snapshot.as_ref().unwrap().scope) {
                    "国际参考"
                } else {
                    self.snapshot.as_ref().unwrap().scope.realm.label()
                },
                q.provider
            ),
            _ => q.quality_at(chrono::Utc::now()).label().into(),
        };
        div()
            .text_sm()
            .text_color(Palette::of(cx).text)
            .overflow_hidden()
            .text_ellipsis()
            .whitespace_nowrap()
            .child(text)
    }
    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let element = div().id(("quote-row", row));
        // The table also asks the delegate to render filler rows below the data.
        let quote = self.snapshot.as_ref().and_then(|snapshot| {
            self.rows
                .get(row)
                .and_then(|index| snapshot.quotes.get(*index))
        });
        match quote {
            Some(quote) => element.aria_label(format!(
                "{}，{}，{}",
                quote.name,
                quote.display_price(),
                category_label(&quote.category)
            )),
            None => element,
        }
    }
}

impl PriceTable {
    pub(crate) fn sort_rows(&mut self) {
        let Some((column, direction)) = self.sort else {
            return;
        };
        if direction == ColumnSort::Default {
            self.rows.sort_unstable();
            return;
        }
        if let Some(snapshot) = &self.snapshot {
            self.rows.sort_by(|a, b| {
                let a = &snapshot.quotes[*a];
                let b = &snapshot.quotes[*b];
                let order = match column {
                    0 => a.name.cmp(&b.name),
                    1 => a.unit.cmp(&b.unit).then(a.amount.cmp(&b.amount)),
                    2 => a.category.cmp(&b.category),
                    3 => a.provider.cmp(&b.provider),
                    _ => a.quality.label().cmp(b.quality.label()),
                }
                .then(a.item_id.cmp(&b.item_id));
                if direction == ColumnSort::Descending {
                    order.reverse()
                } else {
                    order
                }
            });
        }
    }
}
