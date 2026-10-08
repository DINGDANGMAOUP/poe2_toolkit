use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use poe2_core::{GameId, League, MarketScope, MarketSnapshot, Quality, Quote, Realm, digest};
use reqwest::{Client, Url};
use rust_decimal::Decimal;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

mod china;
mod poe1;
mod tablets;
mod uniques;

const SCOUT: &str = "https://api.poe2scout.com";
const MAX_RESPONSE: usize = 32 * 1024 * 1024;

#[derive(Debug)]
pub struct LeagueDiscovery {
    pub leagues: Vec<League>,
    pub warnings: Vec<String>,
}

#[derive(Clone)]
pub struct MarketClient {
    client: Client,
    cache: Arc<tokio::sync::Mutex<BTreeMap<String, CachedResponse>>>,
    retry_after: Arc<tokio::sync::Mutex<BTreeMap<String, std::time::Instant>>>,
}

#[derive(Clone)]
struct CachedResponse {
    value: Value,
    etag: Option<String>,
    validated: std::time::Instant,
    bytes: usize,
}

impl MarketClient {
    pub fn new() -> Result<Self> {
        Ok(Self {
            cache: Default::default(),
            retry_after: Default::default(),
            client: Client::builder()
                .user_agent(concat!(
                    "PoE-Toolkit/",
                    env!("CARGO_PKG_VERSION"),
                    " (desktop economy client)"
                ))
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::limited(3))
                .build()?,
        })
    }

    async fn get(&self, url: Url) -> Result<Value> {
        self.get_checked(url, false).await
    }
    async fn get_checked(&self, url: Url, revalidate: bool) -> Result<Value> {
        let key = url.as_str().to_owned();
        let origin = url.origin().ascii_serialization();
        if let Some(until) = self.retry_after.lock().await.get(&origin).copied() {
            ensure!(
                std::time::Instant::now() >= until,
                "行情服务限流，仍需等待 {} 秒",
                until
                    .saturating_duration_since(std::time::Instant::now())
                    .as_secs()
            );
        }
        let cached = self.cache.lock().await.get(&key).cloned();
        if let Some(entry) = &cached
            && !revalidate
            && entry.validated.elapsed() < Duration::from_secs(300)
        {
            return Ok(entry.value.clone());
        }
        let mut request = self.client.get(url);
        if let Some(tag) = cached.as_ref().and_then(|c| c.etag.as_ref()) {
            request = request.header(reqwest::header::IF_NONE_MATCH, tag);
        }
        let mut response = request.send().await.context("行情服务连接失败")?;
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let seconds = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| {
                    s.parse::<u64>().ok().or_else(|| {
                        DateTime::parse_from_rfc2822(s).ok().map(|at| {
                            (at.with_timezone(&Utc) - Utc::now()).num_seconds().max(1) as u64
                        })
                    })
                })
                .unwrap_or(60)
                .clamp(1, 86400);
            self.retry_after.lock().await.insert(
                origin,
                std::time::Instant::now() + Duration::from_secs(seconds),
            );
            bail!("行情服务限流，请在 {seconds} 秒后重试");
        }
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            let mut entry = cached.context("来源返回 304 但没有已验证缓存")?;
            entry.validated = std::time::Instant::now();
            self.cache.lock().await.insert(key, entry.clone());
            return Ok(entry.value);
        }
        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        response = response
            .error_for_status()
            .context("行情服务返回错误状态")?;
        ensure!(
            response.content_length().unwrap_or(0) <= MAX_RESPONSE as u64,
            "行情响应超过大小上限"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= MAX_RESPONSE,
                "行情响应超过大小上限"
            );
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes).context("行情服务响应不是有效 JSON")?;
        let mut cache = self.cache.lock().await;
        while !cache.is_empty()
            && (cache.len() >= 64
                || cache.values().map(|e| e.bytes).sum::<usize>() + bytes.len() > 64 * 1024 * 1024)
        {
            let oldest = cache
                .iter()
                .min_by_key(|(_, e)| e.validated)
                .map(|(k, _)| k.clone())
                .unwrap();
            cache.remove(&oldest);
        }
        cache.insert(
            key,
            CachedResponse {
                value: value.clone(),
                etag,
                validated: std::time::Instant::now(),
                bytes: bytes.len(),
            },
        );
        Ok(value)
    }

    pub async fn discover_leagues(&self) -> Result<LeagueDiscovery> {
        self.discover_leagues_for(GameId::Poe2).await
    }
    pub async fn discover_leagues_for(&self, game: GameId) -> Result<LeagueDiscovery> {
        let (international, china) = tokio::join!(
            self.leagues_for_game(game, Realm::International),
            self.leagues_for_game(game, Realm::China)
        );
        let mut discovery = LeagueDiscovery {
            leagues: Vec::new(),
            warnings: Vec::new(),
        };
        for (realm, result) in [(Realm::International, international), (Realm::China, china)] {
            match result {
                Ok(leagues) => discovery.leagues.extend(leagues),
                Err(error) => discovery
                    .warnings
                    .push(format!("{}赛季获取失败：{error:#}", realm.label())),
            }
        }
        ensure!(
            !discovery.leagues.is_empty(),
            "{}",
            discovery.warnings.join("；")
        );
        Ok(discovery)
    }

    pub async fn leagues_for(&self, realm: Realm) -> Result<Vec<League>> {
        self.leagues_for_game(GameId::Poe2, realm).await
    }
    pub async fn leagues_for_game(&self, game: GameId, realm: Realm) -> Result<Vec<League>> {
        if realm == Realm::China {
            return china::parse_leagues_for(
                game,
                &self.get(china::seasons_url_for(china::BASE, game)?).await?,
            );
        }
        if game == GameId::Poe1 {
            return self.poe1_leagues().await;
        }
        self.leagues().await
    }

    pub async fn leagues(&self) -> Result<Vec<League>> {
        let data = self
            .get(Url::parse(&format!("{SCOUT}/poe2/Leagues"))?)
            .await?;
        let rows = data.as_array().context("赛季列表结构不兼容")?;
        let mut leagues = Vec::new();
        for row in rows {
            let id = required_text(row, "ShortName")?;
            let name = required_text(row, "Value")?;
            leagues.push(League {
                game: poe2_core::GameId::Poe2,
                id,
                hardcore: name.starts_with("HC ") || name == "Hardcore",
                name,
                current: row["IsCurrent"].as_bool().unwrap_or(false),
                realm: Realm::International,
            });
        }
        leagues.sort_by_key(|l| (!l.current, l.hardcore));
        ensure!(!leagues.is_empty(), "没有可用赛季");
        Ok(leagues)
    }

    pub async fn refresh(
        &self,
        scope: &MarketScope,
        cancel: Arc<AtomicBool>,
    ) -> Result<MarketSnapshot> {
        self.refresh_with_cache(scope, cancel, None).await
    }
    pub async fn refresh_with_cache(
        &self,
        scope: &MarketScope,
        cancel: Arc<AtomicBool>,
        previous: Option<&MarketSnapshot>,
    ) -> Result<MarketSnapshot> {
        if scope.game == GameId::Poe1 {
            return if scope.realm == Realm::China {
                self.refresh_poe1_china(scope, previous, cancel).await
            } else {
                self.refresh_poe1(scope, previous, cancel).await
            };
        }
        let (base, tablets, uniques) = tokio::join!(
            self.refresh_base(scope, cancel.clone()),
            self.refresh_tablets(scope, cancel.clone()),
            self.refresh_uniques(scope, previous, cancel.clone())
        );
        check_cancel(&cancel)?;
        let mut quotes = Vec::new();
        let mut warnings = Vec::new();
        let mut revisions = Vec::new();
        let mut success = false;
        for (label, result) in [
            ("通货与材料", base),
            ("碑牌", tablets),
            ("传奇装备", uniques),
        ] {
            match result {
                Ok(snapshot) => {
                    success = true;
                    quotes.extend(snapshot.quotes);
                    warnings.extend(snapshot.warnings);
                    revisions.push(snapshot.source_revision);
                }
                Err(error) => warnings.push(format!("{label}未更新：{error:#}")),
            }
        }
        ensure!(success, "{}", warnings.join("；"));
        if let Some(old) = previous.filter(|s| &s.scope == scope) {
            fn group(q: &Quote) -> &str {
                if q.feature() == poe2_core::Feature::Unique {
                    if q.source_market
                        .as_ref()
                        .is_some_and(|s| s.realm == Realm::China)
                    {
                        "通货与材料"
                    } else {
                        "传奇装备"
                    }
                } else if q.item_id.starts_with("tablet:magic:") {
                    "魔法词缀"
                } else if q.item_id.starts_with("tablet:rare:") {
                    "稀有词缀"
                } else if q.feature() == poe2_core::Feature::TabletNames {
                    "碑牌名称"
                } else {
                    "通货与材料"
                }
            }
            for layer in ["通货与材料", "碑牌名称", "魔法词缀", "稀有词缀"] {
                if !quotes.iter().any(|q| group(q) == layer) {
                    let cached: Vec<_> = old
                        .quotes
                        .iter()
                        .filter(|q| group(q) == layer)
                        .cloned()
                        .collect();
                    if !cached.is_empty() {
                        warnings.push(format!("{layer}保留同市场旧数据，来源时间不重置"));
                        quotes.extend(cached);
                    }
                }
            }
        }
        uniques::prefer_domestic(&mut quotes, scope);
        let source_revision = Some(digest(&serde_json::to_vec(&revisions)?));
        let id = digest(&serde_json::to_vec(&(scope, &quotes, &source_revision))?);
        Ok(MarketSnapshot {
            id,
            scope: scope.clone(),
            fetched_at: Utc::now(),
            source_revision,
            quotes,
            warnings,
        })
    }
    async fn refresh_base(
        &self,
        scope: &MarketScope,
        cancel: Arc<AtomicBool>,
    ) -> Result<MarketSnapshot> {
        if scope.realm == Realm::China {
            return self.refresh_china(scope, cancel, china::BASE).await;
        }
        check_cancel(&cancel)?;
        let leagues = self.leagues().await?;
        let league = leagues
            .iter()
            .find(|l| l.id == scope.league && l.hardcore == scope.hardcore)
            .context("赛季身份已失效，请重新选择")?;
        check_cancel(&cancel)?;
        let scout_url = |endpoint: &str| -> Result<Url> {
            let mut url = Url::parse(SCOUT)?;
            url.path_segments_mut()
                .map_err(|_| anyhow::anyhow!("无效基础 URL"))?
                .extend(["poe2", "Leagues", &league.id, endpoint]);
            Ok(url)
        };
        let (pairs, stamp) = tokio::try_join!(
            self.get(scout_url("SnapshotPairs")?),
            self.get(scout_url("ExchangeSnapshot")?)
        )?;
        check_cancel(&cancel)?;
        let fetched_at = Utc::now();
        let mut snapshot = parse_scout(scope.clone(), &pairs, &stamp, fetched_at)?;
        check_cancel(&cancel)?;
        snapshot.quotes.sort_by(|a, b| {
            a.category
                .cmp(&b.category)
                .then(a.name.cmp(&b.name))
                .then(a.item_id.cmp(&b.item_id))
        });
        snapshot.id = digest(&serde_json::to_vec(&(
            &snapshot.scope,
            &snapshot.quotes,
            &snapshot.source_revision,
        ))?);
        Ok(snapshot)
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("操作已取消，未修改游戏文件");
    }
    Ok(())
}

fn required_text(value: &Value, key: &str) -> Result<String> {
    value[key]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() < 2048)
        .map(str::to_owned)
        .with_context(|| format!("来源缺少有效字段 {key}"))
}

fn decimal(value: &Value) -> Option<Decimal> {
    Decimal::from_str(
        value
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| value.to_string())
            .as_str(),
    )
    .ok()
}

pub fn parse_scout(
    scope: MarketScope,
    pairs: &Value,
    stamp: &Value,
    now: DateTime<Utc>,
) -> Result<MarketSnapshot> {
    let rows = pairs.as_array().context("Scout 报价结构不兼容")?;
    ensure!(rows.len() <= 100_000, "报价数量超过上限");
    let base = required_text(stamp, "BaseCurrencyApiId")?;
    let observed_at = stamp["Epoch"]
        .as_i64()
        .and_then(|s| DateTime::from_timestamp(s, 0));
    let mut selected: BTreeMap<String, (Decimal, Quote)> = BTreeMap::new();
    let mut revision: Option<String> = None;
    for row in rows {
        ensure!(
            row["BaseCurrencyApiId"].as_str() == Some(&base),
            "Scout 计价基准不一致"
        );
        let row_revision = row["CurrencyExchangeSnapshotId"]
            .as_u64()
            .map(|x| x.to_string())
            .context("缺少快照标识")?;
        if let Some(expected) = &revision {
            ensure!(expected == &row_revision, "Scout 返回混合快照，拒绝激活");
        } else {
            revision = Some(row_revision);
        }
        for (item_key, price_key) in [
            ("CurrencyOne", "CurrencyOneData"),
            ("CurrencyTwo", "CurrencyTwoData"),
        ] {
            let item = &row[item_key];
            let price = &row[price_key];
            let Some(amount) = decimal(&price["RelativePrice"]).filter(|p| *p > Decimal::ZERO)
            else {
                continue;
            };
            let id = required_text(item, "ApiId")?;
            let activity = decimal(&price["ValueTraded"]).unwrap_or_default();
            let mut reasons = Vec::new();
            let quality =
                if observed_at.is_none_or(|t| t > now || (now - t).num_seconds() > 4 * 3600) {
                    reasons.push("来源时间缺失、异常或超过四小时".into());
                    Quality::Stale
                } else if activity <= Decimal::ZERO {
                    reasons.push("没有可确认的近期交易量".into());
                    Quality::LowEvidence
                } else {
                    Quality::Fresh
                };
            let quote = Quote {
                variant: Default::default(),
                statistic: poe2_core::PriceStatistic::Exchange,
                source_market: Some(scope.clone()),
                item_id: format!("scout:{id}"),
                name: required_text(item, "Text")?,
                category: item["CategoryApiId"].as_str().unwrap_or("currency").into(),
                amount,
                unit: base.clone(),
                provider: "poe2scout".into(),
                observed_at,
                fetched_at: now,
                sample_count: None,
                quality,
                reasons,
                metadata_id: item["BaseItemTypeId"].as_str().map(str::to_owned),
                tablet: None,
            };
            if selected.get(&id).is_none_or(|(old, _)| activity > *old) {
                selected.insert(id, (activity, quote));
            }
        }
    }
    ensure!(!selected.is_empty(), "Scout 没有可用报价，保留上次快照");
    Ok(MarketSnapshot {
        id: String::new(),
        scope,
        fetched_at: now,
        source_revision: revision,
        quotes: selected.into_values().map(|(_, q)| q).collect(),
        warnings: vec![],
    })
}

pub fn parse_ninja(data: &Value, category: &str, now: DateTime<Utc>) -> Result<Vec<Quote>> {
    let unit = required_text(&data["core"], "primary")?;
    let lines = data["lines"].as_array().context("ninja 缺少报价数组")?;
    ensure!(lines.len() <= 100_000, "报价数量超过上限");
    let mut out = Vec::new();
    for line in lines {
        let Some(amount) = decimal(&line["primaryValue"]).filter(|p| *p > Decimal::ZERO) else {
            continue;
        };
        let id = required_text(line, "detailsId")?;
        let count = line["listingCount"].as_u64();
        let mut reasons = vec!["挂牌参考，非成交保证；来源未返回采样时间".into()];
        let quality = if count.is_none_or(|n| n == 0) {
            reasons.push("缺少挂牌样本".into());
            Quality::LowEvidence
        } else {
            Quality::Fresh
        };
        out.push(Quote {
            variant: poe2_core::ItemVariant {
                corrupted: line["corrupted"].as_bool().filter(|v| *v),
                discriminator: line["variant"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
                ..Default::default()
            },
            statistic: poe2_core::PriceStatistic::Listing,
            source_market: None,
            item_id: format!("ninja:{category}:{id}"),
            name: required_text(line, "name")?,
            category: format!("unique:{category}"),
            amount,
            unit: unit.clone(),
            provider: "poe.ninja".into(),
            observed_at: None,
            fetched_at: now,
            sample_count: count,
            quality,
            reasons,
            metadata_id: None,
            tablet: None,
        });
    }
    Ok(out)
}
