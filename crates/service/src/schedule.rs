//! Automatic application is scoped to a successfully reviewed manual plan.
use crate::{Store, transaction, updates};
use anyhow::{Result, ensure};
use poe2_core::{Feature, MarketScope, OperationState, PlanIntent, Profile};
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct Grant {
    game: poe2_core::GameId,
    annotation: poe2_core::AnnotationPolicy,
    installation: String,
    market: Option<MarketScope>,
    features: Vec<Feature>,
}
fn binding(profile: &Profile) -> Option<Grant> {
    let installation = profile.installation.as_ref()?;
    let mut features = profile.features.clone();
    features.sort();
    features.dedup();
    Some(Grant {
        game: profile.game,
        annotation: profile.annotation.clone(),
        installation: installation.id.clone(),
        market: profile.market.clone(),
        features,
    })
}
pub fn validate(store: &Store, profile: &Profile) -> Result<()> {
    ensure!(
        binding(profile).is_some()
            && store.get::<Grant>(&format!(
                "auto_apply_grant:{}:{}",
                profile.game.key(),
                profile.id
            ))? == binding(profile),
        "请先对当前安装、市场与功能方案手动预览并成功应用一次，再开启自动更新补丁"
    );
    ensure!(
        profile.installation.as_ref().is_some_and(|i| i.can_write),
        "当前安装不可写入"
    );
    ensure!(
        poe2_formats::checked_path(
            &profile.installation.as_ref().unwrap().root,
            ".poe2-toolkit-ownership.json"
        )?
        .is_file(),
        "请先手动应用当前方案，再开启自动更新补丁"
    );
    Ok(())
}
pub fn record_manual_success(
    store: &Store,
    plan: &poe2_core::PatchPlan,
    operation: &poe2_core::Operation,
) -> Result<()> {
    if operation.state == OperationState::Committed && matches!(plan.intent, PlanIntent::Annotate) {
        let profile = store.profile_for_game(plan.game)?;
        ensure!(
            profile.revision == plan.profile_revision
                && profile
                    .installation
                    .as_ref()
                    .is_some_and(|i| i.id == plan.installation_id),
            "配置在应用期间改变，自动更新授权未创建"
        );
        if let Some(grant) = binding(&profile) {
            store.set(
                &format!("auto_apply_grant:{}:{}", profile.game.key(), profile.id),
                &grant,
            )?;
        }
    }
    Ok(())
}
pub fn apply_if_enabled(store: &Store, cancel: &AtomicBool) -> Result<Option<String>> {
    let profile = store.profile()?;
    apply_profile_if_enabled(store, cancel, profile)
}
/// A delayed refresh or rules result can only act on the profile that requested it.
pub(crate) fn apply_if_enabled_bound(
    store: &Store,
    cancel: &AtomicBool,
    expected: (poe2_core::GameId, u64),
) -> Result<Option<String>> {
    let profile = store.profile()?;
    if (profile.game, profile.revision) != expected {
        return Ok(None);
    }
    apply_profile_if_enabled(store, cancel, profile)
}
fn apply_profile_if_enabled(
    store: &Store,
    cancel: &AtomicBool,
    profile: Profile,
) -> Result<Option<String>> {
    if !profile.auto_apply {
        return Ok(None);
    }
    validate(store, &profile)?;
    ensure!(
        poe2_formats::checked_path(
            &profile.installation.as_ref().unwrap().root,
            ".poe2-toolkit-ownership.json"
        )?
        .is_file(),
        "自动更新已暂停：当前安装没有本应用的已应用补丁"
    );
    let snapshot = profile
        .market
        .as_ref()
        .map(|s| store.latest_snapshot(s))
        .transpose()?
        .flatten();
    let (version, rules) = updates::active_rules(store)?;
    // Planning expired or missing quotes clears only owned price fields whose
    // evidence has expired; it never invents fresh prices during an outage.
    let plan = poe2_formats::plan_resources(&profile, snapshot.as_ref(), false, &version, &rules)?;
    if plan.mutations.is_empty() {
        return Ok(Some("自动检查完成，补丁无需变化".into()));
    }
    store.save_plan(&plan)?;
    Ok(Some(apply_or_queue(store, &plan.id, false, cancel)?))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingDeployment {
    pub game: poe2_core::GameId,
    pub installation_id: String,
    pub plan_id: String,
    pub profile_revision: u64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub manual: bool,
    pub blocked: Option<String>,
}
pub fn pending(store: &Store) -> Result<Vec<PendingDeployment>> {
    store.values_with_prefix("pending_deployment:")
}
pub fn cancel_pending(store: &Store, installation: &str) -> Result<()> {
    store.remove(&format!("pending_deployment:{installation}"))
}
/// Replaces only this installation's desired target; the saved plan stays immutable.
pub fn apply_or_queue(
    store: &Store,
    id: &str,
    manual: bool,
    cancel: &AtomicBool,
) -> Result<String> {
    let plan = store.plan(id)?;
    let profile = store.profile()?;
    ensure!(
        plan.game == profile.game
            && plan.profile_revision == profile.revision
            && profile
                .installation
                .as_ref()
                .is_some_and(|i| i.id == plan.installation_id),
        "计划上下文已改变，请重新预览"
    );
    if crate::game_guard::is_running(&plan.installation_root)? {
        let queued = PendingDeployment {
            game: plan.game,
            installation_id: plan.installation_id.clone(),
            plan_id: plan.id,
            profile_revision: profile.revision,
            created_at: chrono::Utc::now(),
            manual,
            blocked: None,
        };
        store.set(
            &format!("pending_deployment:{}", queued.installation_id),
            &queued,
        )?;
        return Ok(format!(
            "{} · 已排队，等待游戏退出；关闭预览不会取消任务",
            profile.game.label()
        ));
    }
    let operation = transaction::apply_saved_plan(store, id, cancel)?;
    if manual {
        record_manual_success(store, &plan, &operation)?;
    }
    cancel_pending(store, &plan.installation_id)?;
    store.set(
        &format!("active_preview:{}", plan.game.key()),
        &Option::<String>::None,
    )?;
    Ok(operation.summary)
}
pub fn poll_pending(store: &Store, cancel: &AtomicBool) -> Result<Vec<String>> {
    let mut messages = Vec::new();
    for mut queued in pending(store)? {
        if queued.blocked.is_some() {
            continue;
        }
        let result = (|| -> Result<Option<String>> {
            let plan = store.plan(&queued.plan_id)?;
            let profile = store.profile_for_game(queued.game)?;
            ensure!(
                profile.revision == queued.profile_revision
                    && profile
                        .installation
                        .as_ref()
                        .is_some_and(|i| i.id == queued.installation_id),
                "方案或安装已改变，请重新预览"
            );
            if !queued.manual {
                validate(store, &profile)?;
                ensure!(profile.auto_apply, "自动应用已关闭");
            }
            ensure!(
                plan.expires_at.is_none_or(|at| at > chrono::Utc::now()),
                "价格已到期，请刷新并重新预览"
            );
            if crate::game_guard::is_running(&plan.installation_root)? {
                return Ok(None);
            }
            let operation = transaction::apply_bound_plan(store, &plan.id, cancel, &profile)?;
            if queued.manual
                && operation.state == OperationState::Committed
                && matches!(plan.intent, PlanIntent::Annotate)
                && let Some(grant) = binding(&profile)
            {
                store.set(
                    &format!("auto_apply_grant:{}:{}", profile.game.key(), profile.id),
                    &grant,
                )?;
            }
            cancel_pending(store, &queued.installation_id)?;
            store.set(
                &format!("active_preview:{}", queued.game.key()),
                &Option::<String>::None,
            )?;
            Ok(Some(format!(
                "{} · {}",
                queued.game.label(),
                operation.summary
            )))
        })();
        match result {
            Ok(Some(message)) => messages.push(message),
            Ok(None) => {}
            Err(error) => {
                queued.blocked = Some(format!("{error:#}"));
                store.set(
                    &format!("pending_deployment:{}", queued.installation_id),
                    &queued,
                )?;
                messages.push(format!(
                    "{} 待应用任务需要处理：{error:#}",
                    queued.game.label()
                ));
            }
        }
    }
    Ok(messages)
}
