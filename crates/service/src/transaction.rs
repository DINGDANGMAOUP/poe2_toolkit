//! Recoverable file-set execution. Only explicitly supported resource backends
//! may enter this engine. The journal remains readable without SQLite.
use crate::Store;
use anyhow::{Context, Result, ensure};
use chrono::Utc;
use fs2::FileExt;
use poe2_core::{Operation, OperationState, PatchPlan};
use poe2_formats::{checked_path, hash_file};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    path: String,
    before: Option<String>,
    after: Option<String>,
    backup: Option<String>,
    staged: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Journal {
    version: u32,
    operation: Operation,
    root: PathBuf,
    entries: Vec<Entry>,
}
const ACTIVE_MARKER: &str = ".poe2-toolkit-active.json";

fn clear_active(root: &Path, id: &str) -> Result<()> {
    let path = checked_path(root, ACTIVE_MARKER)?;
    if !path.exists() {
        return Ok(());
    }
    let active: String = serde_json::from_slice(&fs::read(&path)?)?;
    ensure!(active == id, "当前安装的待恢复操作与记录不一致");
    fs::remove_file(path)?;
    #[cfg(unix)]
    File::open(root)?.sync_all()?;
    Ok(())
}

fn durable_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("路径没有父目录")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".poe2-{}.tmp", Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        #[cfg(unix)]
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn save(store: &Store, dir: &Path, journal: &Journal) -> Result<()> {
    durable_write(
        &dir.join("journal.json"),
        &serde_json::to_vec_pretty(journal)?,
    )?;
    store.save_operation(&journal.operation)
}

fn lock(root: &Path) -> Result<File> {
    let path = checked_path(root, ".poe2-toolkit-operation.lock")?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    file.try_lock_exclusive()
        .context("此安装有另一个操作正在执行")?;
    Ok(file)
}

fn current_hash(path: &Path) -> Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            ensure!(
                meta.is_file() && !meta.file_type().is_symlink(),
                "目标不是普通文件"
            );
            Ok(Some(hash_file(path)?))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn execute(store: &Store, plan: &PatchPlan, cancel: &AtomicBool) -> Result<Operation> {
    execute_inner(store, plan, cancel)
}

/// Shared policy entry point for both desktop and CLI.
pub fn apply_saved_plan(store: &Store, id: &str, cancel: &AtomicBool) -> Result<Operation> {
    let profile = store.profile()?;
    apply_bound_plan(store, id, cancel, &profile)
}
pub(crate) fn apply_bound_plan(
    store: &Store,
    id: &str,
    cancel: &AtomicBool,
    profile: &poe2_core::Profile,
) -> Result<Operation> {
    let plan = store.plan(id)?;
    ensure!(
        profile.game == plan.game
            && profile.valid_context()
            && profile.revision == plan.profile_revision
            && (plan.scope.is_none() || profile.market == plan.scope),
        "配置或市场已改变，请重新预览"
    );
    ensure!(
        profile
            .installation
            .as_ref()
            .is_some_and(|i| i.id == plan.installation_id),
        "所选安装与预览不一致"
    );
    if matches!(plan.intent, poe2_core::PlanIntent::Annotate) {
        let (version, rules) = crate::updates::active_rules(store)?;
        ensure!(plan.rule_version == version, "规则已更新或失效，请重新预览");
        if let Some(snapshot_id) = &plan.snapshot_id {
            ensure!(
                profile.market_matches_client(),
                "行情市场与目标客户端服区不一致，请重新预览"
            );
            let scope = plan.scope.as_ref().context("价格计划缺少市场身份")?;
            let snapshot = store
                .snapshot_by_id(snapshot_id)
                .context("行情快照不存在")?;
            ensure!(
                snapshot.scope == *scope
                    && snapshot.id == *snapshot_id
                    && !plan.quote_ids.is_empty(),
                "行情已改变，请重新预览"
            );
            for id in &plan.quote_ids {
                ensure!(
                    snapshot
                        .quotes
                        .iter()
                        .any(|q| &q.item_id == id && rules.permits(q, Utc::now())),
                    "计划引用的报价已过期或失效，请重新预览"
                );
            }
        } else {
            ensure!(
                plan.quote_ids.is_empty() && plan.scope.is_none(),
                "无行情计划含有价格依赖"
            );
        }
    }
    let operation = execute(store, &plan, cancel)?;
    if operation.state == OperationState::Committed
        && matches!(plan.intent, poe2_core::PlanIntent::ClearAnnotations)
    {
        store.disable_auto_apply(plan.game)?;
    }
    Ok(operation)
}

fn execute_inner(store: &Store, plan: &PatchPlan, cancel: &AtomicBool) -> Result<Operation> {
    let _maintenance = crate::maintenance::acquire(store)?;
    ensure!(plan.protocol == 3 && plan.verify_seal(), "补丁计划校验失败");
    ensure!(
        plan.expires_at.is_none_or(|at| at > Utc::now()),
        "计划价格已到期，请重新预览"
    );
    ensure!(
        plan.scope.as_ref().is_none_or(|s| s.game == plan.game),
        "计划游戏身份不一致"
    );
    ensure!(plan.blockers.is_empty(), "计划存在未解决冲突");
    ensure!(!plan.mutations.is_empty(), "计划没有需要写入的变化");
    ensure!(plan.mutations.len() <= 128, "单次写入文件数量超出上限");
    ensure!(!cancel.load(Ordering::Relaxed), "操作已取消");
    crate::game_guard::ensure_stopped(&plan.installation_root)?;
    let _lock = lock(&plan.installation_root)?;
    for previous in journals(store)? {
        if previous.operation.plan_id == plan.id
            && previous.operation.state == OperationState::Committed
        {
            for entry in &previous.entries {
                ensure!(
                    current_hash(&checked_path(&previous.root, &entry.path)?)? == entry.after,
                    "本计划已应用，但之后资源发生变化"
                );
            }
            return Ok(previous.operation);
        }
    }
    let installation = poe2_formats::inspect_for_game(&plan.installation_root, plan.game)?;
    ensure!(
        installation.can_write && installation.id == plan.installation_id,
        "安装身份或写入能力不匹配"
    );
    ensure!(
        installation.fingerprint == plan.installation_fingerprint,
        "资源在预览后发生变化，请重新生成计划"
    );
    ensure!(
        !checked_path(&plan.installation_root, ACTIVE_MARKER)?.exists(),
        "此安装存在待恢复操作，请使用原数据目录完成恢复"
    );
    validate_read_set(plan)?;
    let _source_handles = hold_sources(plan)?;
    for journal in journals(store)? {
        ensure!(
            journal.operation.installation_id != plan.installation_id
                || matches!(
                    journal.operation.state,
                    OperationState::Committed
                        | OperationState::RolledBack
                        | OperationState::FailedUnchanged
                ),
            "存在未结束操作，请先恢复"
        );
    }
    let mut paths = std::collections::BTreeSet::new();
    let required = plan
        .mutations
        .iter()
        .try_fold(0u64, |sum, m| -> Result<u64> {
            let path = checked_path(&plan.installation_root, &m.relative_path)?;
            ensure!(
                paths.insert(m.relative_path.to_lowercase()),
                "计划包含重复目标路径"
            );
            ensure!(
                current_hash(&path)? == m.before_hash,
                "文件在预览后发生变化: {}",
                m.relative_path
            );
            let len = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            ensure!(
                (m.rebuild.is_some() || len <= 512 * 1024 * 1024)
                    && m.content.len() <= 512 * 1024 * 1024,
                "单文件超出事务上限"
            );
            let output_len = if let Some(recipe) = &m.rebuild {
                recipe.output_len().context("容器重建范围无效")?
            } else {
                m.content.len() as u64
            };
            sum.checked_add(len)
                .and_then(|v| v.checked_add(output_len))
                .context("磁盘预算溢出")
        })?;
    let largest_output = plan
        .mutations
        .iter()
        .map(|m| {
            m.rebuild
                .as_ref()
                .and_then(|r| r.output_len())
                .unwrap_or(m.content.len() as u64)
        })
        .max()
        .unwrap_or(0);
    let budget = required
        .checked_add(largest_output)
        .and_then(|v| v.checked_add(16 * 1024 * 1024))
        .context("磁盘预算溢出")?;
    ensure!(
        fs2::available_space(&store.root)? > budget,
        "恢复资料目录磁盘空间不足"
    );
    ensure!(
        fs2::available_space(&plan.installation_root)? > budget,
        "目标磁盘空间不足"
    );
    let now = Utc::now();
    let id = Uuid::new_v4().to_string();
    let dir = store.root.join("transactions").join(&id);
    fs::create_dir_all(&dir)?;
    let operation = Operation {
        snapshot_id: plan.snapshot_id.clone(),
        scope: plan.scope.clone(),
        game: plan.game,
        id,
        plan_id: plan.id.clone(),
        installation_id: plan.installation_id.clone(),
        started_at: now,
        updated_at: now,
        state: OperationState::Preparing,
        summary: "准备恢复资料".into(),
    };
    let mut journal = Journal {
        version: 1,
        operation,
        root: plan.installation_root.clone(),
        entries: Vec::new(),
    };
    save(store, &dir, &journal)?;
    let preparation = (|| -> Result<()> {
        // No target writes occur until *all* backups and staged files are durable.
        for (n, m) in plan.mutations.iter().enumerate() {
            let backup = if let Some(expected) = &m.before_hash {
                let source = checked_path(&journal.root, &m.relative_path)?;
                ensure!(&hash_file(&source)? == expected, "准备期间文件发生变化");
                let name = format!("{n}.before");
                durable_copy(&source, &dir.join(&name))?;
                ensure!(
                    &hash_file(&dir.join(&name))? == expected,
                    "恢复备份复制校验失败"
                );
                Some(name)
            } else {
                None
            };
            let staged = format!("{n}.after");
            if let Some(recipe) = &m.rebuild {
                durable_with(&dir.join(&staged), |output| {
                    let hash = poe2_formats::stream::reconstruct(
                        &checked_path(&journal.root, &m.relative_path)?,
                        recipe,
                        output,
                    )?;
                    ensure!(hash == m.after_hash, "容器重建校验失败");
                    Ok(())
                })?;
            } else {
                durable_write(&dir.join(&staged), &m.content)?;
            }
            journal.entries.push(Entry {
                path: m.relative_path.clone(),
                before: m.before_hash.clone(),
                after: (!m.remove).then(|| m.after_hash.clone()),
                backup,
                staged,
            });
        }
        Ok(())
    })();
    if let Err(error) = preparation {
        journal.operation.state = OperationState::FailedUnchanged;
        journal.operation.summary = format!("准备失败，目标未修改：{error}");
        save(store, &dir, &journal)?;
        return Err(error);
    }
    journal.operation.state = OperationState::Prepared;
    journal.operation.summary = "恢复资料已持久化".into();
    save(store, &dir, &journal)?;
    if cancel.load(Ordering::Relaxed) {
        journal.operation.state = OperationState::FailedUnchanged;
        journal.operation.summary = "已取消，目标未修改".into();
        save(store, &dir, &journal)?;
        return Ok(journal.operation);
    }
    if let Err(error) = validate_read_set(plan)
        .and_then(|()| crate::game_guard::ensure_stopped(&plan.installation_root))
    {
        journal.operation.state = OperationState::FailedUnchanged;
        journal.operation.summary = format!("提交前检查失败，目标未修改：{error}");
        save(store, &dir, &journal)?;
        return Err(error);
    }
    durable_write(
        &checked_path(&journal.root, ACTIVE_MARKER)?,
        &serde_json::to_vec(&journal.operation.id)?,
    )?;
    journal.operation.state = OperationState::Writing;
    journal.operation.summary = "正在提交文件；取消将在安全边界处理".into();
    save(store, &dir, &journal)?;
    let result = (|| -> Result<()> {
        for (n, e) in journal.entries.iter().enumerate() {
            let target = checked_path(&journal.root, &e.path)?;
            ensure!(current_hash(&target)? == e.before, "写入前文件被外部修改");
            if let Some(expected) = &e.after {
                let source = dir.join(&e.staged);
                ensure!(&hash_file(&source)? == expected, "暂存文件损坏");
                durable_target_copy(&source, &target, &journal.operation.id, n)?;
            } else if target.exists() {
                fs::remove_file(&target)?;
                sync_parent(&target)?;
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        journal.operation.state = OperationState::RecoveryRequired;
        journal.operation.updated_at = Utc::now();
        journal.operation.summary = format!("写入中断，需要恢复：{error}");
        save(store, &dir, &journal)?;
        return Ok(journal.operation);
    }
    journal.operation.state = OperationState::Verifying;
    save(store, &dir, &journal)?;
    for e in &journal.entries {
        if current_hash(&checked_path(&journal.root, &e.path)?)? != e.after.clone() {
            journal.operation.state = OperationState::RecoveryRequired;
            journal.operation.summary = "读回校验失败，需要恢复".into();
            save(store, &dir, &journal)?;
            return Ok(journal.operation);
        }
    }
    journal.operation.state = OperationState::Committed;
    journal.operation.updated_at = Utc::now();
    journal.operation.summary = format!(
        "已应用 {} 项字段变化；{} 个文件",
        plan.changes.len(),
        journal.entries.len()
    );
    save(store, &dir, &journal)?;
    clear_active(&journal.root, &journal.operation.id)?;
    Ok(journal.operation)
}

fn journals(store: &Store) -> Result<Vec<Journal>> {
    let base = store.root.join("transactions");
    if !base.exists() {
        return Ok(vec![]);
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(base)? {
        let entry = entry?;
        ensure!(
            entry.file_type()?.is_dir() && !entry.file_type()?.is_symlink(),
            "事务目录包含不合法条目"
        );
        let path = entry.path().join("journal.json");
        if !path.exists() {
            continue;
        }
        ensure!(fs::metadata(&path)?.len() < 1024 * 1024, "恢复日志过大");
        let journal: Journal =
            serde_json::from_slice(&fs::read(path)?).context("恢复日志无法解析，停止后续写入")?;
        ensure!(journal.version == 1, "恢复日志版本不兼容");
        ensure!(
            Uuid::parse_str(&journal.operation.id)?.to_string()
                == entry.file_name().to_string_lossy(),
            "恢复日志身份与目录不符"
        );
        out.push(journal);
    }
    Ok(out)
}

pub fn reconcile(store: &Store) -> Result<Vec<Operation>> {
    let mut operations = Vec::new();
    for mut j in journals(store)? {
        if matches!(
            j.operation.state,
            OperationState::Preparing
                | OperationState::Prepared
                | OperationState::Writing
                | OperationState::Verifying
        ) {
            let _guard = match lock(&j.root) {
                Ok(lock) => lock,
                Err(e)
                    if e.downcast_ref::<std::io::Error>().is_some_and(|e| {
                        e.kind() == std::io::ErrorKind::WouldBlock
                            || e.raw_os_error().is_some_and(|code| {
                                Some(code) == fs2::lock_contended_error().raw_os_error()
                            })
                    }) =>
                {
                    operations.push(j.operation);
                    continue;
                }
                Err(e) => return Err(e),
            };
            j.operation.state = OperationState::RecoveryRequired;
            j.operation.summary = "上次操作未正常结束，请执行恢复检查".into();
            let dir = store.root.join("transactions").join(&j.operation.id);
            save(store, &dir, &j)?;
        } else {
            store.save_operation(&j.operation)?;
            if matches!(
                j.operation.state,
                OperationState::Committed | OperationState::RolledBack
            ) && let Ok(_guard) = lock(&j.root)
            {
                let marker = checked_path(&j.root, ACTIVE_MARKER)?;
                if marker.exists()
                    && serde_json::from_slice::<String>(&fs::read(&marker)?)? == j.operation.id
                {
                    clear_active(&j.root, &j.operation.id)?;
                }
            }
        }
        operations.push(j.operation);
    }
    Ok(operations)
}

pub fn restore_operation(store: &Store, id: &str) -> Result<Operation> {
    let _maintenance = crate::maintenance::acquire(store)?;
    let id = Uuid::parse_str(id)?.to_string();
    let dir = store.root.join("transactions").join(&id);
    ensure!(
        fs::metadata(dir.join("journal.json"))?.len() < 1024 * 1024,
        "恢复日志过大"
    );
    let mut journal: Journal = serde_json::from_slice(&fs::read(dir.join("journal.json"))?)?;
    ensure!(journal.version == 1, "恢复日志版本不兼容");
    ensure!(journal.operation.id == id, "恢复日志身份与目录不符");
    crate::game_guard::ensure_stopped(&journal.root)?;
    let _lock = lock(&journal.root)?;
    ensure!(
        poe2_formats::inspect_for_game(&journal.root, journal.operation.game)?.id
            == journal.operation.installation_id,
        "安装身份发生变化"
    );
    let marker = checked_path(&journal.root, ACTIVE_MARKER)?;
    if marker.exists() {
        ensure!(
            serde_json::from_slice::<String>(&fs::read(&marker)?)? == id,
            "请先恢复此安装尚未结束的操作"
        );
    }
    store.disable_auto_apply(journal.operation.game)?;
    // Check every file and backup before restoring any file.
    ensure!(journal.entries.len() <= 128, "恢复文件数量超过上限");
    let mut paths = std::collections::BTreeSet::new();
    for e in &journal.entries {
        ensure!(
            e.before.is_some() == e.backup.is_some(),
            "恢复日志缺失原始文件的备份引用"
        );
        ensure!(paths.insert(e.path.to_lowercase()), "恢复日志包含重复目标");
        let current = current_hash(&checked_path(&journal.root, &e.path)?)?;
        ensure!(
            current == e.before || current == e.after.clone(),
            "{} 存在外部修改，未执行还原",
            e.path
        );
        if let Some(name) = &e.backup {
            crate::transaction::validate_blob_name(name)?;
            ensure!(
                Some(hash_file(&dir.join(name))?) == e.before,
                "恢复资料损坏"
            );
        }
    }
    journal.operation.state = OperationState::RecoveryRequired;
    journal.operation.summary = "正在恢复上次操作前的状态".into();
    durable_write(&marker, &serde_json::to_vec(&id)?)?;
    save(store, &dir, &journal)?;
    for (n, e) in journal.entries.iter().enumerate().rev() {
        let path = checked_path(&journal.root, &e.path)?;
        let temporary = target_temporary(&path, &id, n)?;
        if temporary.exists() {
            ensure!(
                std::fs::symlink_metadata(&temporary)?.file_type().is_file(),
                "恢复暂存不是普通文件"
            );
            fs::remove_file(&temporary)?;
        }
        let current = current_hash(&path)?;
        if current == e.before {
            continue;
        }
        ensure!(current == e.after.clone(), "恢复期间出现外部修改");
        if let Some(name) = &e.backup {
            let source = dir.join(name);
            ensure!(
                Some(hash_file(&source)?) == e.before,
                "恢复资料在检查后发生变化"
            );
            durable_target_copy(&source, &path, &id, n)?;
        } else {
            fs::remove_file(&path)?;
            sync_parent(&path)?;
        }
        ensure!(
            current_hash(&checked_path(&journal.root, &e.path)?)? == e.before,
            "恢复读回校验失败"
        );
    }
    journal.operation.state = OperationState::RolledBack;
    journal.operation.updated_at = Utc::now();
    journal.operation.summary = "已还原，保留恢复记录".into();
    save(store, &dir, &journal)?;
    clear_active(&journal.root, &journal.operation.id)?;
    Ok(journal.operation)
}

fn validate_blob_name(name: &str) -> Result<()> {
    ensure!(
        Path::new(name).components().count() == 1 && !name.contains(['\\', '/', ':']),
        "恢复资料路径无效"
    );
    Ok(())
}

fn validate_read_set(plan: &PatchPlan) -> Result<()> {
    ensure!(plan.read_set.len() <= 512, "读取依赖数量超出上限");
    for source in &plan.read_set {
        ensure!(
            current_hash(&checked_path(
                &plan.installation_root,
                &source.relative_path
            )?)? == source.hash,
            "读取依赖在预览后发生变化：{}",
            source.relative_path
        );
    }
    Ok(())
}

fn hold_sources(plan: &PatchPlan) -> Result<Vec<File>> {
    let mut handles = Vec::new();
    for entry in &plan.read_set {
        if entry.hash.is_none()
            || plan
                .mutations
                .iter()
                .any(|m| m.relative_path == entry.relative_path)
        {
            continue;
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Deny write and deletion while immutable input bundles are in use.
            options.share_mode(1);
        }
        handles.push(options.open(checked_path(&plan.installation_root, &entry.relative_path)?)?);
    }
    Ok(handles)
}

fn sync_parent(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path.parent().context("文件缺少父目录")?)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn durable_with(path: &Path, write: impl FnOnce(&mut File) -> Result<()>) -> Result<()> {
    let parent = path.parent().context("路径没有父目录")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".poe2-{}.tmp", Uuid::new_v4()));
    durable_with_temporary(path, &temporary, write)
}

fn durable_with_temporary(
    path: &Path,
    temporary: &Path,
    write: impl FnOnce(&mut File) -> Result<()>,
) -> Result<()> {
    fs::create_dir_all(path.parent().context("目标缺少父目录")?)?;
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(temporary)?;
        write(&mut file)?;
        file.sync_all()?;
        drop(file);
        fs::rename(temporary, path)?;
        sync_parent(path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
fn durable_copy(source: &Path, destination: &Path) -> Result<()> {
    durable_with(destination, |target| {
        let mut source = File::open(source)?;
        std::io::copy(&mut source, target)?;
        Ok(())
    })
}
fn target_temporary(path: &Path, id: &str, index: usize) -> Result<PathBuf> {
    let id = Uuid::parse_str(id)?;
    Ok(path
        .parent()
        .context("目标缺少父目录")?
        .join(format!(".poe2-{id}-{index}.tmp")))
}
fn durable_target_copy(source: &Path, target: &Path, id: &str, index: usize) -> Result<()> {
    durable_with_temporary(target, &target_temporary(target, id, index)?, |output| {
        std::io::copy(&mut File::open(source)?, output)?;
        Ok(())
    })
}
