use crate::application_monitor::{
    Mode as ApplicationMode, Schedule as ApplicationSchedule, UserActivity,
};
use crate::update_activity::{UpdateActivity, UpdatePhase, UpdateTarget};
use crate::{MarketClient, Store, transaction};
use anyhow::{Context, Result, ensure};
use chrono::Utc;
use poe2_core::{League, MarketSnapshot, Operation, PatchPlan, Profile};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};

#[derive(Debug, Clone)]
pub struct AppState {
    pub revision: u64,
    pub close_to_tray: bool,
    pub profile: Profile,
    pub leagues: Vec<League>,
    pub league_warnings: Vec<String>,
    pub snapshot: Option<Arc<MarketSnapshot>>,
    pub plan: Option<Arc<PatchPlan>>,
    pub operations: Vec<Operation>,
    pub busy: bool,
    pub refreshing: Option<poe2_core::GameId>,
    pub pending_deployments: Vec<crate::schedule::PendingDeployment>,
    pub status: String,
    pub error: Option<String>,
    pub data_dir: PathBuf,
    pub rule_version: String,
    pub update_configured: bool,
    pub installations: Vec<poe2_core::Installation>,
    pub client_discovery: crate::discovery::DiscoveryReport,
    pub application_update: crate::app_updates::Status,
    pub application_check: UpdateActivity,
    pub application_download_started: bool,
    pub rules_check: UpdateActivity,
}

impl AppState {
    pub fn update_activity(&self, target: UpdateTarget) -> &UpdateActivity {
        match target {
            UpdateTarget::Application => &self.application_check,
            UpdateTarget::Rules => &self.rules_check,
        }
    }

    fn update_activity_mut(&mut self, target: UpdateTarget) -> &mut UpdateActivity {
        match target {
            UpdateTarget::Application => &mut self.application_check,
            UpdateTarget::Rules => &mut self.rules_check,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Command {
    SetCloseToTray(bool),
    SwitchGame(poe2_core::GameId),
    CancelPending(String),
    DiscoverLeagues,
    SaveProfile(Box<Profile>),
    Inspect(PathBuf),
    ConnectDetected(PathBuf),
    Refresh,
    Preview { restore: bool },
    Apply(String),
    Restore(String),
    CheckRules,
    DiscoverInstallations,
    CheckApplication,
    WatchApplicationUpdates,
    InstallApplication,
}

impl Command {
    pub fn update_target(&self) -> Option<UpdateTarget> {
        match self {
            Self::CheckApplication | Self::InstallApplication => Some(UpdateTarget::Application),
            Self::CheckRules => Some(UpdateTarget::Rules),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct Envelope {
    game: poe2_core::GameId,
    revision: u64,
    command: Command,
}

#[derive(Clone)]
pub struct ServiceHandle {
    sender: mpsc::Sender<Envelope>,
    pub state: watch::Receiver<Arc<AppState>>,
    cancel: Arc<AtomicBool>,
    refresh_cancel: Arc<AtomicBool>,
    user_activity: UserActivity,
}

impl ServiceHandle {
    pub fn start(root: &Path) -> Result<Self> {
        let store = Store::open(root)?;
        let profile = store.profile()?;
        let snapshot = profile
            .market
            .as_ref()
            .map(|scope| store.latest_snapshot(scope))
            .transpose()?
            .flatten()
            .map(Arc::new);
        let recovery = transaction::reconcile(&store)
            .err()
            .map(|e| format!("恢复检查失败：{e:#}"));
        let state = AppState {
            revision: 1,
            close_to_tray: store.close_to_tray()?,
            profile,
            leagues: store.get("leagues")?.unwrap_or_default(),
            league_warnings: Vec::new(),
            snapshot,
            plan: None,
            operations: store.operations()?,
            busy: false,
            refreshing: None,
            pending_deployments: crate::schedule::pending(&store)?,
            status: "就绪 · 行情与游戏应用独立更新".into(),
            error: recovery,
            data_dir: store.root.clone(),
            rule_version: crate::updates::active_rules(&store)?.0,
            update_configured: store
                .get::<crate::updates::UpdateSource>("update_source")?
                .is_some(),
            installations: store
                .installation_profiles()?
                .into_iter()
                .filter_map(|p| p.installation)
                .collect(),
            client_discovery: Default::default(),
            application_update: crate::app_updates::status(&store)?,
            application_check: UpdateActivity::default(),
            application_download_started: false,
            rules_check: UpdateActivity::default(),
        };
        let (sender, mut receiver) = mpsc::channel(8);
        let (publish, watch) = watch::channel(Arc::new(state.clone()));
        let cancel = Arc::new(AtomicBool::new(false));
        let cancellation = cancel.clone();
        let refresh_cancel = Arc::new(AtomicBool::new(false));
        let market_cancellation = refresh_cancel.clone();
        let user_activity = UserActivity::default();
        let worker_activity = user_activity.clone();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let client = MarketClient::new()?;
        std::thread::Builder::new()
            .name("poe2-services".into())
            .spawn(move || {
                runtime.block_on(service_loop(
                    store,
                    client,
                    state,
                    &mut receiver,
                    publish,
                    RuntimeSignals {
                        cancel: cancellation,
                        refresh_cancel: market_cancellation,
                        user_activity: worker_activity,
                    },
                ));
            })?;
        Ok(Self {
            sender,
            state: watch,
            cancel,
            refresh_cancel,
            user_activity,
        })
    }
    pub fn send(&self, command: Command) -> Result<()> {
        self.sender
            .try_send(Envelope {
                game: self.state.borrow().profile.game,
                revision: self.state.borrow().profile.revision,
                command,
            })
            .context("任务队列已满或服务已停止")
    }
    pub fn note_user_activity(&self) {
        self.user_activity.touch();
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.refresh_cancel.store(true, Ordering::Relaxed);
    }
    pub fn snapshot(&self) -> Arc<AppState> {
        self.state.borrow().clone()
    }
}

struct RuntimeSignals {
    cancel: Arc<AtomicBool>,
    refresh_cancel: Arc<AtomicBool>,
    user_activity: UserActivity,
}

fn start_application_job(
    store: Store,
    mode: ApplicationMode,
    progress: mpsc::UnboundedSender<(u64, UpdatePhase)>,
    generation: u64,
) -> tokio::task::JoinHandle<Result<String>> {
    tokio::spawn(async move {
        let cancel = AtomicBool::new(false);
        match mode {
            ApplicationMode::Probe => crate::app_updates::probe(&store, &cancel).await,
            ApplicationMode::Download => {
                crate::app_updates::check_with_progress(&store, &cancel, |phase| {
                    let _ = progress.send((generation, phase));
                })
                .await
            }
        }
    })
}

async fn service_loop(
    store: Store,
    client: MarketClient,
    mut state: AppState,
    receiver: &mut mpsc::Receiver<Envelope>,
    publish: watch::Sender<Arc<AppState>>,
    signals: RuntimeSignals,
) {
    let RuntimeSignals {
        cancel,
        refresh_cancel,
        user_activity,
    } = signals;
    let mut application: Option<tokio::task::JoinHandle<Result<String>>> = None;
    let mut application_mode = ApplicationMode::Download;
    let mut application_schedule: Option<ApplicationSchedule> = None;
    let (application_progress, mut progress_events) = mpsc::unbounded_channel();
    let mut ticker = tokio::time::interval(Duration::from_secs(10));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut retry_after = Instant::now();
    let mut rules_after = Instant::now();
    let mut refresh: Option<tokio::task::JoinHandle<Result<(poe2_core::GameId, u64, usize)>>> =
        None;
    let mut refresh_binding = None;
    loop {
        let command = tokio::select! {
            result=async { match &mut application { Some(job)=>job.await, None=>std::future::pending().await } }=>{
                application=None;
                let result = result.map_err(anyhow::Error::from).and_then(|r| r);
                if let Some(schedule) = &mut application_schedule {
                    schedule.attempted(Instant::now(), result.is_ok());
                }
                if application_mode == ApplicationMode::Download {
                    state.application_check.finish(result.map_err(|error| update_error(&error)));
                } else if let Ok(message) = result {
                    state.application_check.finish(Ok(message));
                    state.application_download_started = false;
                }
                // Probe failures stay quiet and preserve any previously verified availability.
                // Progress events from a completed task must not revive its running state.
                while progress_events.try_recv().is_ok() {}
                if let Ok(status) = crate::app_updates::status(&store) { state.application_update=status; }
                emit(&publish,&mut state);
                continue;
            },
            Some((generation, phase))=progress_events.recv()=>{
                if application.is_some() && generation == state.application_check.generation {
                    if matches!(phase, UpdatePhase::Downloading { .. } | UpdatePhase::Verifying) {
                        state.application_download_started=true;
                    }
                    state.application_check.phase=phase;
                    emit(&publish,&mut state);
                }
                continue;
            },
            result=async { match &mut refresh { Some(job)=>job.await, None=>std::future::pending().await } }=>{
                refresh=None;state.refreshing=None;
                let expected=refresh_binding.take();
                match result {
                    Ok(Ok((game,revision,count)))=>{
                        state.status=format!("{} 已刷新 {count} 条行情；补丁尚未更新",game.label());
                        if game==state.profile.game && revision==state.profile.revision && state.profile.auto_apply {
                            state.busy=true;emit(&publish,&mut state);
                            let db=store.clone();let cancellation=cancel.clone();
                            match tokio::task::spawn_blocking(move||crate::schedule::apply_if_enabled_bound(&db,&cancellation,(game,revision))).await {
                                Ok(Ok(Some(message)))=>state.status.push_str(&format!("；{message}")),
                                Ok(Err(error))=>state.status.push_str(&format!("；应用暂停：{error:#}")),
                                Err(error)=>state.error=Some(error.to_string()),_=>{}
                            }
                            state.busy=false;
                        }
                    },
                    Ok(Err(error))=>{
                        state.error=Some(format!("行情未刷新：{error:#}"));retry_after=Instant::now()+Duration::from_secs(300);
                        if state.profile.auto_apply && expected==Some((state.profile.game,state.profile.revision)) {
                            state.busy=true;emit(&publish,&mut state);
                            let db=store.clone();let cancellation=cancel.clone();
                            if let Some(expected)=expected {
                                match tokio::task::spawn_blocking(move||crate::schedule::apply_if_enabled_bound(&db,&cancellation,expected)).await {
                                    Ok(Ok(Some(message)))=>state.status=format!("行情获取失败；{message}"),
                                    Ok(Err(error))=>state.status=format!("行情获取失败；应用暂停：{error:#}"),
                                    Err(error)=>state.error=Some(error.to_string()),_=>{}
                                }
                            }
                            state.busy=false;
                        }
                    },
                    Err(error)=>state.error=Some(error.to_string()),
                }
                if let Err(error)=reload_local_state(&store,&mut state){state.error=Some(error.to_string());}
                emit(&publish,&mut state);continue;
            },
            command=receiver.recv()=>match command {Some(envelope)=>{
                if !matches!(envelope.command, Command::SetCloseToTray(_) | Command::CheckApplication | Command::InstallApplication | Command::WatchApplicationUpdates)
                    && (envelope.game != state.profile.game || envelope.revision != state.profile.revision) {
                    state.error = Some("任务来自已改变的游戏或方案，请重新操作".into());
                    if let Some(target) = envelope.command.update_target() {
                        let activity = state.update_activity_mut(target);
                        activity.start(UpdatePhase::Checking);
                        activity.finish(Err("游戏或方案已改变，请重新检查更新".into()));
                    }
                    state.busy = false;
                    emit(&publish, &mut state);
                    continue;
                }
                user_activity.touch();
                envelope.command
            },None=>break},
            _=ticker.tick()=>{
                if let Err(error)=reload_local_state(&store,&mut state) {state.error=Some(format!("本地状态加载失败：{error:#}"));}
                if state.pending_deployments.iter().any(|task|task.blocked.is_none()) {
                    state.busy=true;emit(&publish,&mut state);
                    let db=store.clone();let cancellation=cancel.clone();
                    match tokio::task::spawn_blocking(move||crate::schedule::poll_pending(&db,&cancellation)).await {
                        Ok(Ok(messages)) if !messages.is_empty()=>state.status=messages.join("；"),
                        Ok(Err(error))=>state.error=Some(format!("待应用检查失败：{error:#}")),
                        Err(error)=>state.error=Some(error.to_string()),_=>{}
                    }
                    state.busy=false;
                    if let Err(error)=reload_local_state(&store,&mut state){state.error=Some(error.to_string());}
                }
                emit(&publish,&mut state);
                if state.application_update.configured
                    && let Some(mode) = application_schedule.as_ref().and_then(|schedule| schedule.due(
                        Instant::now(), user_activity.idle_for(),
                        application.is_none() && refresh.is_none() && receiver.is_empty()
                            && !state.pending_deployments.iter().any(|task| task.blocked.is_none()),
                        state.application_update.pending_version.is_some(),
                    )) {
                    application_mode = mode;
                    if mode == ApplicationMode::Download {
                        state.application_check.start(UpdatePhase::Checking);
                        state.application_download_started = false;
                    }
                    application = Some(start_application_job(store.clone(), mode, application_progress.clone(), state.application_check.generation));
                    emit(&publish,&mut state);
                }
                if state.update_configured && Instant::now()>=rules_after {
                    rules_after=Instant::now()+Duration::from_secs(3600);
                    Command::CheckRules
                } else {
                    let age=state.snapshot.as_ref().map(|s|s.age_seconds(Utc::now())).unwrap_or(i64::MAX);
                    if refresh.is_some() || !state.profile.auto_refresh || state.profile.market.is_none() || Instant::now()<retry_after
                        || age<i64::from(state.profile.refresh_minutes.max(15))*60 {continue;}
                    Command::Refresh
                }
            }
        };
        if matches!(command, Command::WatchApplicationUpdates) {
            application_schedule.get_or_insert_with(|| ApplicationSchedule::new(Instant::now()));
            emit(&publish, &mut state);
            continue;
        }
        if matches!(command, Command::CheckApplication) {
            if application.is_some() && application_mode == ApplicationMode::Download {
                emit(&publish, &mut state);
                continue;
            }
            // A manual request takes precedence over a metadata-only probe.
            if let Some(probe) = application.take() {
                probe.abort();
            }
            application_mode = ApplicationMode::Download;
            state.application_check.start(UpdatePhase::Checking);
            state.application_download_started = false;
            application = Some(start_application_job(
                store.clone(),
                application_mode,
                application_progress.clone(),
                state.application_check.generation,
            ));
            emit(&publish, &mut state);
            continue;
        }
        if matches!(command, Command::InstallApplication) && application.is_some() {
            emit(&publish, &mut state);
            continue;
        }
        if matches!(command, Command::Refresh) {
            if refresh.is_some() {
                state.status = "已有行情请求正在进行".into();
                emit(&publish, &mut state);
                continue;
            }
            let Some(scope) = state.profile.market.clone() else {
                state.error = Some("请先选择赛季".into());
                emit(&publish, &mut state);
                continue;
            };
            let previous = state.snapshot.clone();
            let client = client.clone();
            let db = store.clone();
            let cancellation = refresh_cancel.clone();
            let revision = state.profile.revision;
            refresh_binding = Some((scope.game, revision));
            refresh_cancel.store(false, Ordering::Relaxed);
            state.refreshing = Some(scope.game);
            state.error = None;
            state.status = format!("正在获取 {} 行情，可切换工作区", scope.game.label());
            refresh = Some(tokio::spawn(async move {
                let snapshot = client
                    .refresh_with_cache(&scope, cancellation, previous.as_deref())
                    .await?;
                let count = snapshot.quotes.len();
                tokio::task::spawn_blocking(move || db.save_snapshot(&snapshot)).await??;
                Ok((scope.game, revision, count))
            }));
            emit(&publish, &mut state);
            continue;
        }
        cancel.store(false, Ordering::Relaxed);
        state.busy = true;
        state.error = None;
        state.status = label(&command).into();
        let update_target = command.update_target();
        if let Some(target) = update_target {
            state.update_activity_mut(target).start(
                if matches!(command, Command::InstallApplication) {
                    UpdatePhase::Installing
                } else {
                    UpdatePhase::Checking
                },
            );
        }
        emit(&publish, &mut state);
        match run_command(command, &store, &client, &mut state, cancel.clone()).await {
            Ok(message) => state.status = message,
            Err(error) => {
                state.error = Some(if update_target.is_some() {
                    update_error(&error)
                } else {
                    format!("{error:#}")
                });
                state.status = "操作未完成 · 查看原因后重试".into();
                retry_after = Instant::now() + Duration::from_secs(300);
            }
        }
        if let Err(error) = reload_local_state(&store, &mut state) {
            state.error = Some(format!("本地状态加载失败：{error:#}"));
        }
        if let Some(target) = update_target {
            let result = match &state.error {
                Some(error) => Err(error.clone()),
                None => Ok(state.status.clone()),
            };
            state.update_activity_mut(target).finish(result);
        }
        state.busy = false;
        emit(&publish, &mut state);
    }
    if let Some(job) = application {
        job.abort();
    }
    if let Some(job) = refresh {
        job.abort();
    }
}

fn update_error(error: &anyhow::Error) -> String {
    if let Some(network) = error.downcast_ref::<reqwest::Error>() {
        if network.is_timeout() {
            "连接更新服务器超时，请检查网络后重试。".into()
        } else if let Some(status) = network.status() {
            format!(
                "更新服务器暂时不可用（HTTP {}），请稍后重试。",
                status.as_u16()
            )
        } else {
            "无法连接更新服务器，请检查网络或代理设置后重试。".into()
        }
    } else {
        format!("{error:#}")
    }
}

fn saved_preview(
    store: &Store,
    profile: &Profile,
    version: &str,
) -> Result<Option<Arc<PatchPlan>>> {
    let id: Option<String> = store
        .get::<Option<String>>(&format!("active_preview:{}", profile.game.key()))?
        .flatten();
    Ok(id
        .and_then(|id| store.plan(&id).ok())
        .filter(|p| {
            p.game == profile.game
                && p.profile_revision == profile.revision
                && p.rule_version == version
                && p.verify_seal()
                && p.expires_at.is_none_or(|at| at > Utc::now())
                && profile
                    .installation
                    .as_ref()
                    .is_some_and(|i| i.id == p.installation_id)
        })
        .map(Arc::new))
}
fn reload_local_state(store: &Store, state: &mut AppState) -> Result<()> {
    state.close_to_tray = store.close_to_tray()?;
    let mut profile = store.profile_config()?;
    let snapshot = profile
        .market
        .as_ref()
        .map(|scope| store.latest_snapshot(scope))
        .transpose()?
        .flatten()
        .map(Arc::new);
    let (version, _) = crate::updates::active_rules(store)?;
    if profile.game != state.profile.game
        || profile.revision != state.profile.revision
        || version != state.rule_version
    {
        state.plan = None;
    }
    if let Some(plan) = &state.plan {
        let active: Option<String> = store
            .get::<Option<String>>(&format!("active_preview:{}", profile.game.key()))?
            .flatten();
        if active.as_ref() != Some(&plan.id) || plan.expires_at.is_some_and(|at| at <= Utc::now()) {
            state.plan = None;
        }
    }
    // A preview probes the current resource index. The saved installation is
    // only the last connection-time probe and may predate our previous write.
    // Expose the fresh probe for this exact active binding without changing
    // profile revision, permissions, or persisted configuration. Apply still
    // independently reprobes the files and validates the complete read set.
    if let Some(plan) = &state.plan
        && plan.game == profile.game
        && plan.profile_revision == profile.revision
        && let Some(installation) = &mut profile.installation
        && installation.id == plan.installation_id
        && installation.root == plan.installation_root
    {
        installation.fingerprint = plan.installation_fingerprint.clone();
    }
    state.profile = profile;
    state.snapshot = snapshot;
    state.rule_version = version;
    state.update_configured = store
        .get::<crate::updates::UpdateSource>("update_source")?
        .is_some();
    state.operations = store.operations()?;
    state.pending_deployments = crate::schedule::pending(store)?;
    state.application_update = crate::app_updates::status(store)?;
    Ok(())
}

fn emit(sender: &watch::Sender<Arc<AppState>>, state: &mut AppState) {
    state.revision += 1;
    sender.send_replace(Arc::new(state.clone()));
}
fn label(command: &Command) -> &'static str {
    match command {
        Command::SetCloseToTray(_) => "正在保存应用设置",
        Command::CancelPending(_) => "正在取消待应用任务",
        Command::SwitchGame(_) => "正在切换游戏工作区",
        Command::DiscoverLeagues => "正在获取可用赛季",
        Command::SaveProfile(_) => "正在保存方案",
        Command::Inspect(_) | Command::ConnectDetected(_) => "正在识别客户端并检查资源结构",
        Command::Refresh => "正在刷新行情 · 保留最后有效快照",
        Command::Preview { .. } => "正在生成修改预览",
        Command::Apply(_) => "正在应用已审阅的计划",
        Command::Restore(_) => "正在检查并还原",
        Command::CheckRules => "正在验证规则更新",
        Command::DiscoverInstallations => "正在查找已安装客户端",
        Command::CheckApplication => "正在检查并验证程序升级包",
        Command::WatchApplicationUpdates => "已启用后台程序更新检查",
        Command::InstallApplication => "正在备份数据并准备重启升级",
    }
}

async fn run_command(
    command: Command,
    store: &Store,
    client: &MarketClient,
    state: &mut AppState,
    cancel: Arc<AtomicBool>,
) -> Result<String> {
    match command {
        Command::SetCloseToTray(enabled) => {
            store.set_close_to_tray(enabled)?;
            state.close_to_tray = enabled;
            Ok("应用设置已保存，对所有游戏生效".into())
        }
        Command::CancelPending(id) => {
            crate::schedule::cancel_pending(store, &id)?;
            Ok("待应用任务已取消；已写入资源未改变".into())
        }
        Command::SwitchGame(game) => {
            state.profile = store.switch_game(game, state.profile.revision)?;
            state.plan = saved_preview(store, &state.profile, &state.rule_version)?;
            state.league_warnings.clear();
            Ok(format!("已切换至 {} · 独立市场与补丁方案", game.label()))
        }
        Command::CheckApplication | Command::WatchApplicationUpdates => {
            unreachable!("application downloads run independently of foreground commands")
        }
        Command::InstallApplication => {
            let db = store.clone();
            tokio::task::spawn_blocking(move || crate::app_updates::install(&db)).await??;
            Ok("正在重启安装".into())
        }
        Command::DiscoverLeagues => {
            let discovery = client.discover_leagues_for(state.profile.game).await?;
            merge_leagues(&mut state.leagues, discovery.leagues);
            state.league_warnings = discovery.warnings;
            store.set("leagues", &state.leagues)?;
            Ok("已加载赛季，请选择市场".into())
        }
        Command::SaveProfile(profile) => {
            let profile = *profile;
            ensure!(
                profile.revision == state.profile.revision,
                "配置已被其他操作更新，请重新加载"
            );
            ensure!(
                (15..=1440).contains(&profile.refresh_minutes),
                "刷新间隔须为 15–1440 分钟"
            );
            if profile.auto_apply && !state.profile.auto_apply {
                crate::schedule::validate(store, &profile)?;
            }
            if profile.market != state.profile.market
                && let Some(scope) = &profile.market
            {
                ensure!(
                    state.leagues.iter().any(|league| league.game == scope.game
                        && league.realm == scope.realm
                        && league.id == scope.league
                        && league.hardcore == scope.hardcore),
                    "所选服区与赛季不匹配，请重新获取赛季"
                );
            }
            state.profile = store.update_profile(profile, state.profile.revision)?;
            state.plan = None;
            state.snapshot = state
                .profile
                .market
                .as_ref()
                .map(|s| store.latest_snapshot(s))
                .transpose()?
                .flatten()
                .map(Arc::new);
            Ok("方案已保存，旧预览已失效".into())
        }
        Command::Inspect(_) | Command::ConnectDetected(_) => {
            let auto_switch = matches!(command, Command::ConnectDetected(_));
            let path = match command {
                Command::Inspect(path) | Command::ConnectDetected(path) => path,
                _ => unreachable!(),
            };
            let game = state.profile.game;
            let installation = tokio::task::spawn_blocking(move || {
                let installation = crate::discovery::inspect(&path)?;
                ensure!(
                    auto_switch || installation.game == game,
                    "识别到 {} 客户端；当前为 {}，请先切换游戏工作区再选择。",
                    installation.game.label(),
                    game.label()
                );
                Ok::<_, anyhow::Error>(installation)
            })
            .await??;
            if installation.game != game {
                state.profile = store.switch_game(installation.game, state.profile.revision)?;
                state.plan = None;
            }
            let game = installation.game;
            let summary = format!(
                "已连接 {} · {} · {} · {}",
                game.label(),
                installation
                    .client_realm
                    .map(|r| r.label())
                    .unwrap_or("服区待确认"),
                installation.client_kind.label(),
                if installation.can_write {
                    "资源结构已通过检查"
                } else {
                    "暂不可写，详见资源检查结果"
                }
            );
            state.profile = store.select_installation(installation)?;
            state.snapshot = state
                .profile
                .market
                .as_ref()
                .map(|s| store.latest_snapshot(s))
                .transpose()?
                .flatten()
                .map(Arc::new);
            state.plan = None;
            state.installations = store
                .installation_profiles()?
                .into_iter()
                .filter_map(|p| p.installation)
                .collect();
            Ok(summary)
        }
        Command::Refresh => unreachable!("refresh runs independently of foreground commands"),
        Command::Preview { restore } => {
            let snapshot = state.snapshot.clone();
            let profile = state.profile.clone();
            let (version, rules) = crate::updates::active_rules(store)?;
            let plan = tokio::task::spawn_blocking(move || {
                poe2_formats::plan_resources(
                    &profile,
                    snapshot.as_deref(),
                    restore,
                    &version,
                    &rules,
                )
            })
            .await??;
            store.save_plan(&plan)?;
            store.set(
                &format!("active_preview:{}", plan.game.key()),
                &Some(plan.id.clone()),
            )?;
            let count = plan.changes.len();
            state.plan = Some(Arc::new(plan));
            Ok(format!("预览完成：{count} 项字段变化"))
        }
        Command::Apply(id) => {
            let db = store.clone();
            let message = tokio::task::spawn_blocking(move || {
                crate::schedule::apply_or_queue(&db, &id, true, &cancel)
            })
            .await??;
            if !message.contains("已排队") {
                state.plan = None;
                store.set(
                    &format!("active_preview:{}", state.profile.game.key()),
                    &Option::<String>::None,
                )?;
            }
            Ok(message)
        }

        Command::Restore(id) => {
            let db = store.clone();
            let operation =
                tokio::task::spawn_blocking(move || transaction::restore_operation(&db, &id))
                    .await??;
            state.plan = None;
            Ok(operation.summary)
        }
        Command::CheckRules => {
            let expected = (state.profile.game, state.profile.revision);
            let release = crate::updates::refresh(store).await?;
            state.plan = None;
            let db = store.clone();
            let applied = tokio::task::spawn_blocking(move || {
                crate::schedule::apply_if_enabled_bound(&db, &cancel, expected)
            })
            .await?;
            let applied = match applied {
                Ok(value) => value,
                Err(error) => return Ok(format!("行情/规则已保存；补丁尚未更新：{error:#}")),
            };
            if let Some(message) = applied {
                return Ok(format!("已启用规则 {}；{message}", release.version));
            }
            Ok(format!(
                "已验证并启用规则 {}；请重新生成预览",
                release.version
            ))
        }
        Command::DiscoverInstallations => {
            let saved = store
                .installation_profiles()?
                .into_iter()
                .filter_map(|p| p.installation.map(|i| i.root))
                .collect();
            state.client_discovery =
                tokio::task::spawn_blocking(move || crate::discovery::discover(saved)).await?;
            let count = state
                .client_discovery
                .clients
                .iter()
                .filter(|c| c.identity.game == state.profile.game)
                .count();
            Ok(format!(
                "找到 {count} 个 {} 客户端；选择后才会连接",
                state.profile.game.label()
            ))
        }
    }
}

// Partial provider failures keep the other realm's cached directory intact.
fn merge_leagues(cached: &mut Vec<League>, discovered: Vec<League>) {
    cached.retain(|old| {
        !discovered
            .iter()
            .any(|new| new.game == old.game && new.realm == old.realm)
    });
    cached.extend(discovered);
}
