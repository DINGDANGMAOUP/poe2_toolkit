use crate::{components::empty, theme::Palette};
use gpui_kit::{
    component::{
        IconName,
        table::{Column, TableDelegate, TableState},
    },
    *,
};
use poe2_core::PatchPlan;
use std::sync::Arc;

pub(crate) struct ChangeTable {
    pub plan: Option<Arc<PatchPlan>>,
}
impl TableDelegate for ChangeTable {
    fn columns_count(&self, _: &App) -> usize {
        4
    }
    fn rows_count(&self, _: &App) -> usize {
        self.plan.as_ref().map_or(0, |p| p.changes.len())
    }
    fn column(&self, n: usize, _: &App) -> Column {
        let (key, label, width) = [
            ("object", "资源对象", 160.),
            ("feature", "功能", 110.),
            ("before", "写入前", 260.),
            ("after", "写入后", 320.),
        ][n];
        Column::new(key, label).width(px(width))
    }
    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let text = self
            .plan
            .as_ref()
            .and_then(|p| p.changes.get(row))
            .map(|c| match column {
                0 => c.record_id.clone(),
                1 => c.feature.label().into(),
                2 => c.before.clone(),
                _ => c.after.clone(),
            })
            .unwrap_or_default();
        div()
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
        let label = self
            .plan
            .as_ref()
            .and_then(|p| p.changes.get(row))
            .map(|c| format!("{}，{}，{}", c.feature.label(), c.record_id, c.after))
            .unwrap_or_default();
        div().id(("change-row", row)).aria_label(label)
    }
    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let (title, detail) = if self.plan.is_some() {
            (
                "本次检查没有字段变化",
                "检查上方的阻断原因与文件变化；没有文件变化时无需写入。",
            )
        } else {
            ("先生成预览", "保存方案后检查资源与实际文字变化。")
        };
        empty(IconName::Eye, title, detail, Palette::of(cx))
    }
}
