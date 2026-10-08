use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use poe2_core::{Feature, GameId, MarketScope, Realm};
use poe2_service::{MarketClient, Store, default_data_dir, transaction};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

#[derive(Parser)]
#[command(version, about = "PoE Toolkit — 行情、补丁与恢复")]
struct Args {
    #[arg(long, global = true, value_enum)]
    game: Option<Game>,
    #[arg(long)]
    data_dir: Option<PathBuf>,
    #[arg(long, global = true, value_enum, default_value = "international")]
    realm: MarketRealm,
    #[command(subcommand)]
    command: Action,
}
#[derive(Clone, Copy, ValueEnum)]
enum Game {
    Poe1,
    Poe2,
}
impl From<Game> for GameId {
    fn from(game: Game) -> Self {
        match game {
            Game::Poe1 => Self::Poe1,
            Game::Poe2 => Self::Poe2,
        }
    }
}
#[derive(Clone, Copy, ValueEnum)]
enum MarketRealm {
    #[value(alias = "intl")]
    International,
    #[value(alias = "cn")]
    China,
}
impl From<MarketRealm> for Realm {
    fn from(realm: MarketRealm) -> Self {
        match realm {
            MarketRealm::International => Self::International,
            MarketRealm::China => Self::China,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum PlanFeature {
    Currency,
    Unique,
    TabletNames,
    TabletAffixes,
    MapHints,
    DivinationCards,
    Materials,
}
impl From<PlanFeature> for Feature {
    fn from(value: PlanFeature) -> Self {
        match value {
            PlanFeature::Currency => Self::Currency,
            PlanFeature::Unique => Self::Unique,
            PlanFeature::TabletNames => Self::TabletNames,
            PlanFeature::TabletAffixes => Self::TabletAffixes,
            PlanFeature::MapHints => Self::MapHints,
            PlanFeature::DivinationCards => Self::DivinationCards,
            PlanFeature::Materials => Self::Materials,
        }
    }
}

#[derive(Subcommand)]
enum Action {
    Profile,
    ConfigureAppUpdates {
        config: PathBuf,
    },
    CheckAppUpdate,
    ImportAppUpdate {
        envelope: PathBuf,
        package: PathBuf,
    },
    InstallAppUpdate,
    AppUpdateStatus,
    Leagues,
    Installations,
    Refresh {
        #[arg(long)]
        league: String,
        #[arg(long)]
        hardcore: bool,
    },
    Prices {
        #[arg(long)]
        league: String,
        #[arg(long)]
        hardcore: bool,
        #[arg(long, default_value = "")]
        search: String,
    },
    Inspect {
        path: PathBuf,
    },
    Plan {
        path: PathBuf,
        #[arg(long)]
        league: Option<String>,
        #[arg(long)]
        hardcore: bool,
        #[arg(long)]
        restore: bool,
        #[arg(long, value_delimiter = ',', value_enum)]
        features: Option<Vec<PlanFeature>>,
    },
    Apply {
        plan_id: String,
        #[arg(long)]
        wait_for_exit: bool,
    },
    Pending,
    CancelPending {
        installation_id: String,
    },
    RetryPending,
    Restore {
        operation_id: String,
    },
    Schedule {
        #[arg(long)]
        auto_refresh: bool,
        #[arg(long)]
        auto_apply: bool,
        #[arg(long, default_value_t = 60)]
        minutes: u32,
    },
    UpdatePatch,
    Operations,
    Recover,
    ConfigureUpdates {
        config: PathBuf,
    },
    CheckRules,
    InstallRules {
        input: PathBuf,
    },
    Rules,
    Backup {
        output: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let store = Store::open(args.data_dir.unwrap_or(default_data_dir()?))?;
    let current = store.profile()?;
    let game = args.game.map(GameId::from).unwrap_or(current.game);
    if game != current.game {
        store.switch_game(game, current.revision)?;
    }
    match args.command {
        Action::Profile => print(&store.profile()?)?,
        Action::ConfigureAppUpdates { config } => {
            let source = serde_json::from_slice(&std::fs::read(config)?)?;
            poe2_service::app_updates::configure(&store, source)?;
            print(&poe2_service::app_updates::status(&store)?)?;
        }
        Action::CheckAppUpdate => {
            print(&poe2_service::app_updates::check(&store, &AtomicBool::new(false)).await?)?
        }
        Action::ImportAppUpdate { envelope, package } => print(
            &poe2_service::app_updates::import(&store, &envelope, &package)?,
        )?,
        Action::InstallAppUpdate => poe2_service::app_updates::install(&store)?,
        Action::AppUpdateStatus => print(&poe2_service::app_updates::status(&store)?)?,
        Action::Installations => print(&poe2_service::discovery::installations())?,
        Action::Leagues => print(
            &MarketClient::new()?
                .leagues_for_game(game, args.realm.into())
                .await?,
        )?,
        Action::Refresh { league, hardcore } => {
            let scope = scope_for(game, args.realm.into(), league, hardcore);
            let mut profile = store.profile()?;
            let revision = profile.revision;
            let snapshot = MarketClient::new()?
                .refresh_with_cache(
                    &scope,
                    Arc::new(AtomicBool::new(false)),
                    store.latest_snapshot(&scope)?.as_ref(),
                )
                .await?;
            store.save_snapshot(&snapshot)?;
            if profile.market.as_ref() != Some(&scope) {
                profile.market = Some(scope);
                store.update_profile(profile, revision)?;
            }
            print(
                &serde_json::json!({"snapshot":snapshot.id,"count":snapshot.quotes.len(),"warnings":snapshot.warnings,"scope":snapshot.scope}),
            )?;
        }
        Action::Prices {
            league,
            hardcore,
            search,
        } => {
            let snapshot = store
                .latest_snapshot(&scope_for(game, args.realm.into(), league, hardcore))?
                .context("没有该市场的缓存")?;
            let query = search.to_lowercase();
            print(
                &snapshot
                    .quotes
                    .iter()
                    .filter(|q| {
                        q.name.to_lowercase().contains(&query)
                            || q.item_id.to_lowercase().contains(&query)
                    })
                    .collect::<Vec<_>>(),
            )?;
        }
        Action::Inspect { path } => print(&poe2_formats::inspect_for_game(&path, game)?)?,
        Action::Plan {
            path,
            league,
            hardcore,
            restore,
            features,
        } => {
            let market = league
                .map(|l| scope_for(game, args.realm.into(), l, hardcore))
                .or(store.profile()?.market);
            let snapshot = market
                .as_ref()
                .map(|s| store.latest_snapshot(s))
                .transpose()?
                .flatten();
            let mut profile = store.profile()?;
            let revision = profile.revision;
            profile.market = market;
            if let Some(features) = features {
                profile.features = features.into_iter().map(Feature::from).collect();
            }
            profile.installation = Some(poe2_formats::inspect_for_game(&path, game)?);
            let profile = store.update_profile(profile, revision)?;
            let (version, rules) = poe2_service::updates::active_rules(&store)?;
            let plan = poe2_formats::plan_resources(
                &profile,
                snapshot.as_ref(),
                restore,
                &version,
                &rules,
            )?;
            store.save_plan(&plan)?;
            print(
                &serde_json::json!({"plan_id":plan.id,"changes":plan.changes,"blockers":plan.blockers,"warnings":plan.warnings,"files":plan.mutations.len(),"layers":plan.layers}),
            )?;
        }
        Action::Pending => print(&poe2_service::schedule::pending(&store)?)?,
        Action::CancelPending { installation_id } => {
            poe2_service::schedule::cancel_pending(&store, &installation_id)?;
            print(&"已取消待应用任务")?;
        }
        Action::RetryPending => print(&poe2_service::schedule::poll_pending(
            &store,
            &AtomicBool::new(false),
        )?)?,
        Action::Apply {
            plan_id,
            wait_for_exit,
        } => {
            if wait_for_exit {
                print(&poe2_service::schedule::apply_or_queue(
                    &store,
                    &plan_id,
                    true,
                    &AtomicBool::new(false),
                )?)?;
                return Ok(());
            }
            let operation =
                transaction::apply_saved_plan(&store, &plan_id, &AtomicBool::new(false))?;
            poe2_service::schedule::record_manual_success(
                &store,
                &store.plan(&plan_id)?,
                &operation,
            )?;
            print(&operation)?;
        }
        Action::Schedule {
            auto_refresh,
            auto_apply,
            minutes,
        } => {
            ensure!((15..=1440).contains(&minutes), "刷新间隔须为 15–1440 分钟");
            let mut profile = store.profile()?;
            if auto_apply {
                poe2_service::schedule::validate(&store, &profile)?;
            }
            let revision = profile.revision;
            profile.auto_refresh = auto_refresh || auto_apply;
            profile.auto_apply = auto_apply;
            profile.refresh_minutes = minutes;
            print(&store.update_profile(profile, revision)?)?;
        }
        Action::UpdatePatch => print(&poe2_service::schedule::apply_if_enabled(
            &store,
            &AtomicBool::new(false),
        )?)?,
        Action::Restore { operation_id } => {
            print(&transaction::restore_operation(&store, &operation_id)?)?
        }
        Action::Operations => print(&store.operations()?)?,
        Action::Recover => print(&transaction::reconcile(&store)?)?,
        Action::ConfigureUpdates { config } => {
            let source = serde_json::from_slice(&read_limited(&config, 64 * 1024)?)?;
            poe2_service::updates::configure(&store, source)?;
            print(&serde_json::json!({"configured":true}))?;
        }
        Action::CheckRules => print(&poe2_service::updates::refresh(&store).await?)?,
        Action::InstallRules { input } => {
            let envelope = serde_json::from_slice(&read_limited(&input, 64 * 1024)?)?;
            print(&poe2_service::updates::activate(&store, &envelope)?)?;
        }
        Action::Rules => {
            let (version, rules) = poe2_service::updates::active_rules(&store)?;
            print(&serde_json::json!({"version":version,"rules":rules}))?;
        }
        Action::Backup { output } => {
            ensure!(!output.exists(), "输出文件已存在");
            store.backup(&output)?;
            print(&serde_json::json!({"backup":"complete"}))?;
        }
    }
    Ok(())
}
fn scope_for(game: GameId, realm: Realm, league: String, hardcore: bool) -> MarketScope {
    MarketScope {
        game,
        realm,
        league,
        hardcore,
    }
}
fn print(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
fn read_limited(path: &std::path::Path, limit: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let f = std::fs::File::open(path)?;
    ensure!(f.metadata()?.len() <= limit, "输入文件超过大小上限");
    let mut bytes = Vec::new();
    f.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "输入文件超过大小上限");
    Ok(bytes)
}
