use anyhow::{Context, Result};
use poe2_core::{GameId, MarketScope, MarketSnapshot, Operation, PatchPlan, Profile};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Store {
    pub root: PathBuf,
}

impl Store {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        std::fs::create_dir_all(&root).context("无法创建应用数据目录")?;
        let store = Self {
            root: std::fs::canonicalize(root)?,
        };
        let conn = store.connect()?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS kv(key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS snapshots(id TEXT PRIMARY KEY, scope TEXT NOT NULL, time TEXT NOT NULL, value TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS snapshots_scope_time ON snapshots(scope,time);
            CREATE TABLE IF NOT EXISTS operations(id TEXT PRIMARY KEY, time TEXT NOT NULL, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS plans(id TEXT PRIMARY KEY, value TEXT NOT NULL);")?;
        Ok(store)
    }
    fn connect(&self) -> Result<Connection> {
        let conn = Connection::open(self.root.join("toolkit.sqlite3"))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(conn)
    }
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let v: Option<String> = self
            .connect()?
            .query_row("SELECT value FROM kv WHERE key=?1", [key], |r| r.get(0))
            .optional()?;
        v.map(|v| serde_json::from_str(&v).map_err(Into::into))
            .transpose()
    }
    pub fn set<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        self.connect()?.execute(
            "INSERT INTO kv VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, serde_json::to_string(value)?],
        )?;
        Ok(())
    }
    pub(crate) fn profile_config(&self) -> Result<Profile> {
        Ok(self.get("profile")?.unwrap_or_default())
    }
    pub fn close_to_tray(&self) -> Result<bool> {
        Ok(self.get("application_close_to_tray")?.unwrap_or(false))
    }
    pub fn set_close_to_tray(&self, enabled: bool) -> Result<()> {
        self.set("application_close_to_tray", &enabled)
    }
    pub fn profile(&self) -> Result<Profile> {
        let mut profile: Profile = self.get("profile")?.unwrap_or_default();
        if let Some(installation) = &mut profile.installation {
            match poe2_formats::inspect_for_game(&installation.root, profile.game) {
                Ok(current) => *installation = current,
                Err(error) => {
                    installation.can_write = false;
                    installation.can_read = false;
                    installation.reasons = vec![format!("资源目录当前不可用：{error}")];
                }
            }
        }
        Ok(profile)
    }
    pub fn update_profile(&self, mut next: Profile, expected: u64) -> Result<Profile> {
        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let json: Option<String> = tx
            .query_row("SELECT value FROM kv WHERE key='profile'", [], |r| r.get(0))
            .optional()?;
        let current: Profile = json
            .map(|s| serde_json::from_str(&s))
            .transpose()?
            .unwrap_or_default();
        anyhow::ensure!(
            current.revision == expected && current.game == next.game && current.id == next.id,
            "配置已由其他窗口或 CLI 更新，请重新加载"
        );
        anyhow::ensure!(next.valid_context(), "方案包含其他游戏的市场、安装或功能");
        if current.installation.as_ref().map(|i| &i.id) != next.installation.as_ref().map(|i| &i.id)
            || current.market != next.market
            || current.features != next.features
            || current.annotation != next.annotation
        {
            next.auto_apply = false;
        }
        next.revision = expected.checked_add(1).context("配置版本溢出")?;
        tx.execute("INSERT INTO kv VALUES ('profile',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[serde_json::to_string(&next)?])?;
        tx.execute(
            "INSERT INTO kv VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![
                format!("game_profile:{}", next.game.key()),
                serde_json::to_string(&next)?
            ],
        )?;
        if let Some(installation) = &next.installation {
            tx.execute(
                "INSERT INTO kv VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![
                    format!("installation_profile:{}", installation.id),
                    serde_json::to_string(&next)?
                ],
            )?;
        }
        tx.commit()?;
        Ok(next)
    }
    /// Switch the active workspace and its saved profile in one transaction.
    pub fn switch_game(&self, game: GameId, expected: u64) -> Result<Profile> {
        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let current: Option<String> = tx
            .query_row("SELECT value FROM kv WHERE key='profile'", [], |r| r.get(0))
            .optional()?;
        let current: Profile = current
            .map(|s| serde_json::from_str(&s))
            .transpose()?
            .unwrap_or_default();
        anyhow::ensure!(current.revision == expected, "配置已更新，请重新切换");
        if current.game == game {
            return Ok(current);
        }
        let target: Option<String> = tx
            .query_row(
                "SELECT value FROM kv WHERE key=?1",
                [format!("game_profile:{}", game.key())],
                |r| r.get(0),
            )
            .optional()?;
        let next: Profile = target
            .map(|s| serde_json::from_str(&s))
            .transpose()?
            .unwrap_or_else(|| Profile::for_game(game));
        anyhow::ensure!(
            next.game == game && next.valid_context(),
            "目标游戏配置身份不符"
        );
        for (key, profile) in [
            (format!("game_profile:{}", current.game.key()), &current),
            (format!("game_profile:{}", game.key()), &next),
            ("profile".into(), &next),
        ] {
            tx.execute(
                "INSERT INTO kv VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, serde_json::to_string(profile)?],
            )?;
        }
        tx.commit()?;
        self.profile()
    }
    pub fn profile_for_game(&self, game: GameId) -> Result<Profile> {
        let active = self.profile()?;
        if active.game == game {
            return Ok(active);
        }
        let mut profile = self
            .get::<Profile>(&format!("game_profile:{}", game.key()))?
            .unwrap_or_else(|| Profile::for_game(game));
        if let Some(installation) = &profile.installation {
            profile.installation = Some(poe2_formats::inspect_for_game(&installation.root, game)?);
        }
        anyhow::ensure!(
            profile.game == game && profile.valid_context(),
            "游戏配置身份不符"
        );
        Ok(profile)
    }
    pub fn values_with_prefix<T: DeserializeOwned>(&self, prefix: &str) -> Result<Vec<T>> {
        let conn = self.connect()?;
        let mut query = conn.prepare("SELECT value FROM kv WHERE key LIKE ?1 ORDER BY key")?;
        let rows = query.query_map([format!("{prefix}%")], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    pub fn remove(&self, key: &str) -> Result<()> {
        self.connect()?
            .execute("DELETE FROM kv WHERE key=?1", [key])?;
        Ok(())
    }
    pub(crate) fn disable_auto_apply(&self, game: GameId) -> Result<()> {
        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let current: Option<String> = tx
            .query_row("SELECT value FROM kv WHERE key='profile'", [], |r| r.get(0))
            .optional()?;
        let current: Profile = current
            .map(|s| serde_json::from_str(&s))
            .transpose()?
            .unwrap_or_default();
        let mut profile = if current.game == game {
            current.clone()
        } else {
            let json: Option<String> = tx
                .query_row(
                    "SELECT value FROM kv WHERE key=?1",
                    [format!("game_profile:{}", game.key())],
                    |r| r.get(0),
                )
                .optional()?;
            json.map(|s| serde_json::from_str(&s))
                .transpose()?
                .unwrap_or_else(|| Profile::for_game(game))
        };
        if profile.auto_apply {
            profile.auto_apply = false;
            profile.revision = profile.revision.checked_add(1).context("配置版本溢出")?;
            let value = serde_json::to_string(&profile)?;
            let mut keys = vec![format!("game_profile:{}", game.key())];
            if current.game == game {
                keys.push("profile".into());
            }
            if let Some(installation) = &profile.installation {
                keys.push(format!("installation_profile:{}", installation.id));
            }
            for key in keys {
                tx.execute("INSERT INTO kv VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,value])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    pub fn snapshot_by_id(&self, id: &str) -> Result<MarketSnapshot> {
        let json: String =
            self.connect()?
                .query_row("SELECT value FROM snapshots WHERE id=?1", [id], |r| {
                    r.get(0)
                })?;
        Ok(serde_json::from_str(&json)?)
    }
    pub fn installation_profiles(&self) -> Result<Vec<Profile>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare("SELECT value FROM kv WHERE key LIKE 'installation_profile:%' ORDER BY key")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    pub fn select_installation(&self, installation: poe2_core::Installation) -> Result<Profile> {
        let current = self.profile()?;
        let mut next = self
            .get::<Profile>(&format!("installation_profile:{}", installation.id))?
            .unwrap_or_else(|| current.clone());
        anyhow::ensure!(
            installation.game == current.game && next.game == current.game,
            "所选客户端属于另一款游戏"
        );
        next.id = current.id.clone();
        // Selecting an installation must not silently replace the market being
        // browsed. Planning still validates the market against the target realm.
        next.market = current.market.clone();
        next.installation = Some(installation);
        next.auto_apply = false;
        self.update_profile(next, current.revision)
    }
    pub fn save_snapshot(&self, snapshot: &MarketSnapshot) -> Result<()> {
        self.connect()?.execute(
            "INSERT OR REPLACE INTO snapshots VALUES (?1,?2,?3,?4)",
            params![
                snapshot.id,
                serde_json::to_string(&snapshot.scope)?,
                snapshot.fetched_at.to_rfc3339(),
                serde_json::to_string(snapshot)?
            ],
        )?;
        Ok(())
    }
    pub(crate) fn activate_rules(
        &self,
        envelope: &crate::updates::SignedRules,
        release: &crate::updates::RuleRelease,
    ) -> Result<()> {
        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let key = format!("rule_sequence:{}", release.channel);
        let source: String =
            tx.query_row("SELECT value FROM kv WHERE key='update_source'", [], |r| {
                r.get(0)
            })?;
        let checked = crate::updates::verify(
            &serde_json::from_str(&source)?,
            envelope,
            chrono::Utc::now(),
        )?;
        anyhow::ensure!(
            checked.sequence == release.sequence && checked.channel == release.channel,
            "发行源在验证期间发生变化"
        );
        let current: Option<String> = tx
            .query_row("SELECT value FROM kv WHERE key=?1", [&key], |r| r.get(0))
            .optional()?;
        let current = current
            .map(|s| serde_json::from_str::<u64>(&s))
            .transpose()?
            .unwrap_or(0);
        anyhow::ensure!(release.sequence > current, "规则序号未增加，拒绝重放或降级");
        for (key, value) in [
            (key, serde_json::to_string(&release.sequence)?),
            ("active_rules".into(), serde_json::to_string(envelope)?),
            (
                format!("rule_history:{}:{}", release.channel, release.sequence),
                serde_json::to_string(envelope)?,
            ),
        ] {
            tx.execute(
                "INSERT INTO kv VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, value],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn latest_snapshot(&self, scope: &MarketScope) -> Result<Option<MarketSnapshot>> {
        let value: Option<String> = self
            .connect()?
            .query_row(
                "SELECT value FROM snapshots WHERE scope=?1 ORDER BY time DESC LIMIT 1",
                [serde_json::to_string(scope)?],
                |r| r.get(0),
            )
            .optional()?;
        value
            .map(|v| serde_json::from_str(&v).map_err(Into::into))
            .transpose()
    }
    pub fn save_plan(&self, plan: &PatchPlan) -> Result<()> {
        self.connect()?.execute(
            "INSERT OR REPLACE INTO plans VALUES (?1,?2)",
            params![plan.id, serde_json::to_string(plan)?],
        )?;
        Ok(())
    }
    pub fn plan(&self, id: &str) -> Result<PatchPlan> {
        let value: String = self
            .connect()?
            .query_row("SELECT value FROM plans WHERE id=?1", [id], |r| r.get(0))
            .context("补丁计划不存在")?;
        Ok(serde_json::from_str(&value)?)
    }
    pub fn save_operation(&self, operation: &Operation) -> Result<()> {
        self.connect()?.execute(
            "INSERT OR REPLACE INTO operations VALUES (?1,?2,?3)",
            params![
                operation.id,
                operation.started_at.to_rfc3339(),
                serde_json::to_string(operation)?
            ],
        )?;
        Ok(())
    }
    pub fn operations(&self) -> Result<Vec<Operation>> {
        let conn = self.connect()?;
        let mut statement =
            conn.prepare("SELECT value FROM operations ORDER BY time DESC LIMIT 100")?;
        let values = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        values
            .into_iter()
            .map(|s| serde_json::from_str(&s).map_err(Into::into))
            .collect()
    }
    pub fn backup(&self, target: &Path) -> Result<()> {
        self.connect()?.backup("main", target, None)?;
        Ok(())
    }

    pub fn check_app_release(
        &self,
        envelope: &crate::updates::SignedRules,
        release: &crate::app_updates::AppRelease,
    ) -> Result<()> {
        let key = format!("app_release:{}:{}", release.channel, release.target);
        Self::check_release_record(self.get(&key)?, envelope, release)
    }
    fn check_release_record(
        old: Option<(u64, String, chrono::DateTime<chrono::Utc>)>,
        envelope: &crate::updates::SignedRules,
        release: &crate::app_updates::AppRelease,
    ) -> Result<()> {
        if let Some((sequence, hash, time)) = old {
            anyhow::ensure!(
                release.sequence >= sequence
                    && (release.sequence != sequence
                        || hash == poe2_core::digest(envelope.payload.as_bytes())),
                "程序发行记录回放或序号冲突"
            );
            anyhow::ensure!(
                chrono::Utc::now() + chrono::Duration::minutes(5) >= time,
                "系统时间早于上次验证时间，请校准后重试"
            );
        }
        Ok(())
    }
    pub fn accept_app_release(
        &self,
        envelope: &crate::updates::SignedRules,
        release: &crate::app_updates::AppRelease,
    ) -> Result<()> {
        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let key = format!("app_release:{}:{}", release.channel, release.target);
        let old: Option<String> = tx
            .query_row("SELECT value FROM kv WHERE key=?1", [&key], |r| r.get(0))
            .optional()?;
        Self::check_release_record(
            old.map(|s| serde_json::from_str(&s)).transpose()?,
            envelope,
            release,
        )?;
        let source_json: Option<String> = tx
            .query_row(
                "SELECT value FROM kv WHERE key='app_update_source'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let source = match source_json {
            Some(json) => serde_json::from_str(&json)?,
            None => crate::app_updates::default_source()?,
        };
        crate::app_updates::verify(&source, envelope, chrono::Utc::now())?;
        tx.execute(
            "INSERT INTO kv VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![
                key,
                serde_json::to_string(&(
                    release.sequence,
                    poe2_core::digest(envelope.payload.as_bytes()),
                    chrono::Utc::now()
                ))?
            ],
        )?;
        tx.execute("INSERT INTO kv VALUES ('pending_app_update',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [serde_json::to_string(envelope)?])?;
        tx.commit()?;
        Ok(())
    }
}

pub fn default_data_dir() -> Result<PathBuf> {
    directories::ProjectDirs::from("dev", "poe2-toolkit", "POE2 Toolkit")
        .map(|p| p.data_local_dir().to_path_buf())
        .context("无法确定应用数据目录，请显式指定 --data-dir")
}
