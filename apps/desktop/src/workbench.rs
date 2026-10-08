//! Product status derives from service evidence, never from the last UI click.
use chrono::{DateTime, Utc};
use poe2_core::{Feature, OperationState, PatchPlan, Profile, Quality};
use poe2_service::AppState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NextStep {
    Client,
    Market,
    Refresh,
    Preview,
    Review,
    History,
    Edit,
    None,
}

pub(crate) struct WorkbenchStatus {
    pub title: &'static str,
    pub detail: String,
    pub action: NextStep,
    pub label: &'static str,
    pub preparation: bool,
}

pub(crate) fn preview_matches(
    plan: &PatchPlan,
    profile: &Profile,
    rules: &str,
    now: DateTime<Utc>,
) -> bool {
    plan.game == profile.game
        && plan.profile_revision == profile.revision
        && plan.rule_version == rules
        && plan.expires_at.is_none_or(|at| at > now)
        && plan
            .scope
            .as_ref()
            .is_none_or(|scope| profile.market.as_ref() == Some(scope))
        && profile.installation.as_ref().is_some_and(|i| {
            i.id == plan.installation_id && i.fingerprint == plan.installation_fingerprint
        })
}

pub(crate) fn status(state: &AppState, now: DateTime<Utc>) -> WorkbenchStatus {
    let profile = &state.profile;
    let result = |title, detail: String, action, label, preparation| WorkbenchStatus {
        title,
        detail,
        action,
        label,
        preparation,
    };
    let operations = || {
        state.operations.iter().filter(|o| {
            o.game == profile.game
                && profile
                    .installation
                    .as_ref()
                    .is_some_and(|i| i.id == o.installation_id)
        })
    };
    if operations().any(|o| o.state == OperationState::RecoveryRequired) {
        return result(
            "资源需要检查与恢复",
            "先检查操作记录与恢复结果，再继续生成补丁。".into(),
            NextStep::History,
            "查看恢复详情",
            false,
        );
    }
    if state.busy {
        return result(
            "正在处理任务",
            state.status.clone(),
            NextStep::None,
            "处理中…",
            false,
        );
    }
    if let Some(task) = state.pending_deployments.iter().find(|t| {
        t.game == profile.game
            && profile
                .installation
                .as_ref()
                .is_some_and(|i| i.id == t.installation_id)
    }) {
        return result(
            "补丁等待应用",
            task.blocked
                .clone()
                .unwrap_or_else(|| "等待游戏退出后复核并应用；可在上方取消任务。".into()),
            NextStep::History,
            "查看操作记录",
            false,
        );
    }
    if let Some(plan) = &state.plan {
        if !preview_matches(plan, profile, &state.rule_version, now) {
            return result(
                "预览已失效",
                "目标、方案、规则或有效期发生变化，请重新生成。".into(),
                NextStep::Preview,
                "重新生成预览",
                false,
            );
        }
        if !plan.blockers.is_empty() {
            return result(
                "预览中有待处理的问题",
                plan.blockers.join("；"),
                NextStep::Review,
                "检查预览",
                false,
            );
        }
        if plan.mutations.is_empty() {
            return result(
                "本次检查无需写入",
                "当前预览没有资源变更。".into(),
                NextStep::Review,
                "查看预览结果",
                false,
            );
        }
        return result(
            "预览已生成，尚未写入",
            format!(
                "{} 项变化 · {} 个文件；审阅实际内容后应用。",
                plan.changes.len(),
                plan.mutations.len()
            ),
            NextStep::Review,
            "审阅变化",
            false,
        );
    }
    let Some(installation) = &profile.installation else {
        return result(
            "准备价格标注",
            "选择目标客户端，再准备对应市场的行情；也可以独立浏览行情。".into(),
            NextStep::Client,
            "选择客户端",
            true,
        );
    };
    if !installation.can_write {
        return result(
            "客户端资源需要检查",
            installation.reasons.join("；"),
            NextStep::Client,
            "检查客户端",
            true,
        );
    }
    let needs_market = profile.features.iter().any(|f| *f != Feature::MapHints);
    if profile.features.is_empty() {
        return result(
            "尚未选择补丁功能",
            "在当前方案中选择需要的标注功能。".into(),
            NextStep::Edit,
            "编辑方案",
            true,
        );
    }
    if needs_market {
        if profile.market.is_none() {
            return result(
                "选择行情市场",
                "选择服区和赛季后获取价格。".into(),
                NextStep::Market,
                "选择赛季",
                true,
            );
        }
        if !profile.market_matches_client() {
            return result(
                "市场与客户端服区不同",
                "仍可浏览当前行情；生成价格补丁前请选择匹配市场。".into(),
                NextStep::Market,
                "选择匹配市场",
                true,
            );
        }
        let Some(snapshot) = &state.snapshot else {
            return result(
                "尚未获取当前市场行情",
                "刷新会保留已有资源标注，应用需另行审阅。".into(),
                NextStep::Refresh,
                "获取行情",
                true,
            );
        };
        if snapshot.quotes.is_empty()
            || !snapshot
                .quotes
                .iter()
                .any(|q| q.quality_at(now) == Quality::Fresh)
        {
            return result(
                "暂无有效报价",
                "报价可能已过期、缺失或证据不足；请刷新后检查。".into(),
                NextStep::Refresh,
                "刷新行情",
                true,
            );
        }
        if let Some(operation) = operations().next()
            && operation.state == OperationState::Committed
            && operation.snapshot_id.as_deref() != Some(snapshot.id.as_str())
        {
            return result(
                "行情已更新，可检查变化",
                "新快照不一定产生资源变更，生成预览后确认。".into(),
                NextStep::Preview,
                "检查更新",
                false,
            );
        }
    }
    if let Some(operation) = operations().next() {
        match operation.state {
            OperationState::FailedUnchanged => {
                return result(
                    "上次操作失败，资源未改变",
                    operation.summary.clone(),
                    NextStep::History,
                    "检查失败原因",
                    false,
                );
            }
            OperationState::RolledBack => {
                return result(
                    "上次操作已回滚",
                    operation.summary.clone(),
                    NextStep::History,
                    "查看回滚记录",
                    false,
                );
            }
            _ => {}
        }
    }
    if operations()
        .next()
        .is_some_and(|o| o.state == OperationState::Committed)
    {
        result(
            "补丁已应用",
            "可重新启动游戏；需要更新标注时刷新行情。".into(),
            NextStep::Preview,
            "检查更新",
            false,
        )
    } else {
        result(
            "可以准备补丁预览",
            "预览将检查资源身份、报价条件和实际文字变化。".into(),
            NextStep::Preview,
            "生成预览",
            false,
        )
    }
}
