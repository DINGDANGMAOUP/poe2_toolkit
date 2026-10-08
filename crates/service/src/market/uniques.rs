//! Equipment quotes have their own source market, including cross-realm references.
use super::*;
use poe2_core::Feature;
use std::collections::BTreeSet;

const NINJA: &str = "https://poe.ninja/poe2/api/economy/stash/current/item/overview";
const CATEGORIES: [&str; 7] = [
    "UniqueWeapons",
    "UniqueArmours",
    "UniqueAccessories",
    "UniqueFlasks",
    "UniqueCharms",
    "UniqueJewels",
    "UniqueSanctumRelics",
];

fn league_key(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn resolve<'a>(
    target: &MarketScope,
    domestic: &[League],
    international: &'a [League],
) -> Result<&'a League> {
    if target.realm == Realm::International {
        return international
            .iter()
            .find(|l| l.id == target.league && l.hardcore == target.hardcore)
            .context("传奇来源赛季已失效");
    }
    let selected = domestic
        .iter()
        .find(|l| l.id == target.league && l.hardcore == target.hardcore)
        .context("国服赛季身份已失效，未获取国际参考价")?;
    let key = league_key(&selected.id);
    let matching: Vec<_> = international
        .iter()
        .filter(|l| {
            l.hardcore == target.hardcore
                && (league_key(&l.id) == key || league_key(&l.name) == key)
        })
        .collect();
    ensure!(matching.len() <= 1, "国际参考赛季对应关系不唯一");
    if let Some(league) = matching.first() {
        return Ok(league);
    }
    ensure!(
        selected.current,
        "该国服历史赛季没有对应国际赛季，未改用当前赛季"
    );
    // Scout can list multiple IsCurrent seasons. Respect its published order;
    // preserve and disclose the exact selected season rather than rename it to CN.
    international
        .iter()
        .find(|l| l.current && l.hardcore == target.hardcore)
        .context("来源没有可用的国际参考赛季")
}

fn source_scope(league: &League) -> MarketScope {
    MarketScope {
        game: poe2_core::GameId::Poe2,
        realm: Realm::International,
        league: league.id.clone(),
        hardcore: league.hardcore,
    }
}

impl MarketClient {
    pub(super) async fn refresh_uniques(
        &self,
        target: &MarketScope,
        previous: Option<&MarketSnapshot>,
        cancel: Arc<AtomicBool>,
    ) -> Result<MarketSnapshot> {
        check_cancel(&cancel)?;
        let domestic = if target.realm == Realm::China {
            china::parse_leagues_for(
                target.game,
                &self
                    .get(china::seasons_url_for(china::BASE, target.game)?)
                    .await?,
            )?
        } else {
            Vec::new()
        };
        let international = self.leagues().await?;
        let league = resolve(target, &domestic, &international)?;
        let source = source_scope(league);
        let now = Utc::now();
        let mut quotes = Vec::new();
        let mut warnings = Vec::new();
        for batch in CATEGORIES.chunks(2) {
            check_cancel(&cancel)?;
            let fetch = |category: &'static str| async move {
                let mut url = Url::parse(NINJA)?;
                url.query_pairs_mut()
                    .extend_pairs([("league", league.name.as_str()), ("type", category)]);
                parse_ninja(&self.get(url).await?, category, now)
            };
            let mut results = Vec::new();
            if batch.len() == 2 {
                let (a, b) = tokio::join!(fetch(batch[0]), fetch(batch[1]));
                results.extend([(batch[0], a), (batch[1], b)]);
            } else {
                results.push((batch[0], fetch(batch[0]).await));
            }
            for (category, result) in results {
                match result {
                    Ok(rows) => quotes.extend(rows),
                    Err(e) => warnings.push(format!("{category} 来源 poe.ninja 未更新：{e:#}")),
                }
            }
        }
        // Discover categories from the provider, instead of freezing its item catalog.
        match self.scout_uniques(league, &cancel).await {
            Ok((rows, notes)) => {
                let now = Utc::now();
                warnings.extend(notes);
                let preferred: BTreeSet<_> = quotes
                    .iter()
                    .filter(|q| q.quality_at(now) == Quality::Fresh)
                    .map(|q| q.name.clone())
                    .collect();
                let fallback: BTreeSet<_> = rows
                    .iter()
                    .filter(|q| q.quality_at(now) == Quality::Fresh)
                    .map(|q| q.name.clone())
                    .collect();
                quotes.retain(|q| preferred.contains(&q.name) || !fallback.contains(&q.name));
                quotes.extend(rows.into_iter().filter(|q| !preferred.contains(&q.name)));
            }
            Err(e) => warnings.push(format!("传奇补充来源 poe2scout 未更新：{e:#}")),
        }
        for quote in &mut quotes {
            quote.source_market = Some(source.clone());
            if target.realm == Realm::China {
                quote.reasons.push(format!(
                    "国际服参考价 · {}（{}）· {}；保留国际服原币种，非国服本地报价",
                    league.name,
                    league.id,
                    if league.hardcore { "硬核" } else { "普通" }
                ));
            }
        }
        // Re-resolve both directories after I/O so a visible season switch cannot
        // activate references selected against the old directory.
        check_cancel(&cancel)?;
        let domestic_after = if target.realm == Realm::China {
            china::parse_leagues_for(
                target.game,
                &self
                    .get(china::seasons_url_for(china::BASE, target.game)?)
                    .await?,
            )?
        } else {
            Vec::new()
        };
        let international_after = self.leagues().await?;
        let confirmed = resolve(target, &domestic_after, &international_after)?;
        ensure!(
            source_scope(confirmed) == source
                && confirmed.name == league.name
                && domestic.first().map(|l| &l.id) == domestic_after.first().map(|l| &l.id),
            "采价期间赛季目录变化，未激活传奇参考价"
        );
        retain_cache(&mut quotes, previous, target, &source, &mut warnings);
        check_cancel(&cancel)?;
        ensure!(
            !quotes.is_empty(),
            "传奇来源没有可用报价：{}",
            warnings.join("；")
        );
        if target.realm == Realm::China {
            warnings.push(format!(
                "传奇装备含国际服参考价：{}（{}），保留来源币种；国服已有有效报价优先。",
                league.name, league.id
            ));
        }
        let revision = digest(&serde_json::to_vec(&(&source, &quotes))?);
        Ok(MarketSnapshot {
            id: revision.clone(),
            scope: target.clone(),
            fetched_at: now,
            source_revision: Some(revision),
            quotes,
            warnings,
        })
    }

    async fn scout_uniques(
        &self,
        league: &League,
        cancel: &AtomicBool,
    ) -> Result<(Vec<Quote>, Vec<String>)> {
        let mut base = Url::parse(SCOUT)?;
        base.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("无效传奇来源"))?
            .extend(["poe2", "Leagues", &league.id, ""]);
        let data = self.get(base.join("Items/Categories")?).await?;
        let categories = data["UniqueCategories"]
            .as_array()
            .context("Scout 传奇目录结构不兼容")?;
        ensure!(categories.len() <= 64, "Scout 传奇分类数量超过上限");
        let mut quotes = Vec::new();
        let mut warnings = Vec::new();
        let mut seen = BTreeSet::new();
        for category in categories {
            check_cancel(cancel)?;
            let id = required_text(category, "ApiId")?;
            ensure!(seen.insert(id.clone()), "Scout 传奇分类重复");
            // Tablet and map prices are handled by their independent markets.
            if matches!(id.as_str(), "map" | "tablet") {
                continue;
            }
            match self.scout_unique_category(&base, &id, cancel).await {
                Ok(rows) => quotes.extend(rows),
                Err(e) => warnings.push(format!("Scout 传奇分类 {id} 未更新：{e:#}")),
            }
        }
        Ok((quotes, warnings))
    }

    async fn scout_unique_category(
        &self,
        base: &Url,
        category: &str,
        cancel: &AtomicBool,
    ) -> Result<Vec<Quote>> {
        let mut quotes = Vec::new();
        let mut seen = BTreeSet::new();
        let mut expected = None;
        let mut read = 0;
        for page in 1..=100_u64 {
            check_cancel(cancel)?;
            let mut url = base.join("Uniques/ByCategory")?;
            url.query_pairs_mut().extend_pairs([
                ("Category", category),
                ("ReferenceCurrency", "exalted"),
                ("Page", &page.to_string()),
                ("PerPage", "100"),
                ("DataPoints", "7"),
                ("FrequencyHours", "24"),
            ]);
            let data = self.get(url).await?;
            let rows = data["Items"].as_array().context("Scout 传奇页没有 Items")?;
            let pages = data["Pages"].as_u64().context("Scout 传奇页缺少总页数")?;
            let total = data["Total"].as_u64().context("Scout 传奇页缺少总数")?;
            ensure!(
                data["CurrentPage"].as_u64() == Some(page)
                    && pages <= 100
                    && total <= 10_000
                    && rows.len() <= 100
                    && (pages >= page || total == 0 && page == 1),
                "Scout 传奇分页不兼容"
            );
            if let Some(old) = expected {
                ensure!(
                    old == (pages, total),
                    "Scout 传奇分页发生变化，未合并不完整目录"
                );
            }
            expected = Some((pages, total));
            for row in rows {
                let id = row["UniqueItemId"].as_u64().context("Scout 传奇身份缺失")?;
                ensure!(seen.insert(id), "Scout 传奇分页返回重复身份");
            }
            read += rows.len() as u64;
            quotes.extend(parse_scout_uniques(rows, category, Utc::now())?);
            if page >= pages {
                ensure!(read == total, "Scout 传奇目录未完整返回");
                return Ok(quotes);
            }
            ensure!(!rows.is_empty(), "Scout 传奇中间页为空");
        }
        bail!("Scout 传奇分页超过上限")
    }
}

fn parse_scout_uniques(rows: &[Value], category: &str, now: DateTime<Utc>) -> Result<Vec<Quote>> {
    let category_name = match category {
        "weapon" => "UniqueWeapons",
        "armour" => "UniqueArmours",
        "accessory" => "UniqueAccessories",
        "flask" => "UniqueFlasks",
        "charm" => "UniqueCharms",
        "jewel" => "UniqueJewels",
        "sanctum" => "UniqueSanctumRelics",
        other => other,
    };
    let mut out = Vec::new();
    for row in rows {
        ensure!(
            row["CategoryApiId"].as_str() == Some(category),
            "Scout 传奇分类不符"
        );
        let Some(amount) = decimal(&row["CurrentPrice"]).filter(|x| *x > Decimal::ZERO) else {
            continue;
        };
        let id = row["UniqueItemId"].as_u64().context("Scout 传奇身份缺失")?;
        let count = row["CurrentQuantity"].as_u64();
        out.push(Quote {
            variant: Default::default(),
            statistic: Default::default(),
            source_market: None,
            tablet: None,
            item_id: format!("scout:unique:{id}"),
            name: required_text(row, "Name")?,
            category: format!("unique:{category_name}"),
            amount,
            unit: "exalted".into(),
            provider: "poe2scout".into(),
            observed_at: None,
            fetched_at: now,
            sample_count: count,
            quality: if count.is_some_and(|n| n > 0) {
                Quality::Fresh
            } else {
                Quality::LowEvidence
            },
            reasons: vec![
                "传奇挂牌参考，非成交保证；来源未返回采样时间；请求计价单位为 exalted".into(),
            ],
            metadata_id: None,
        });
    }
    Ok(out)
}

fn retain_cache(
    quotes: &mut Vec<Quote>,
    previous: Option<&MarketSnapshot>,
    target: &MarketScope,
    source: &MarketScope,
    warnings: &mut Vec<String>,
) {
    let Some(old) = previous.filter(|s| &s.scope == target) else {
        return;
    };
    let present: BTreeSet<_> = quotes.iter().map(|q| q.category.clone()).collect();
    let mut retained = 0;
    for q in &old.quotes {
        if q.feature() == Feature::Unique
            && (q.source_market.as_ref() == Some(source)
                || source == target && q.source_market.is_none())
            && !present.contains(&q.category)
        {
            let mut cached = q.clone();
            cached.source_market = Some(source.clone());
            quotes.push(cached);
            retained += 1;
        }
    }
    if retained > 0 {
        warnings.push(format!(
            "传奇来源部分不可用，保留 {retained} 条同来源市场旧报价，采集时间不重置"
        ));
    }
}

fn identity(q: &Quote) -> String {
    q.item_id
        .strip_prefix("poecurrency:")
        .and_then(|s| serde_json::from_str::<(String, String)>(s).ok())
        .map(|(_, name)| name)
        .unwrap_or_else(|| q.name.clone())
        .trim()
        .to_lowercase()
}

pub(super) fn prefer_domestic(quotes: &mut Vec<Quote>, target: &MarketScope) {
    if target.realm != Realm::China {
        return;
    }
    let references: BTreeMap<_, _> = quotes
        .iter()
        .filter(|q| q.feature() == Feature::Unique && q.is_reference_for(target))
        .map(|q| (identity(q), q.category.clone()))
        .collect();
    let mut native = BTreeSet::new();
    for q in quotes
        .iter_mut()
        .filter(|q| !q.is_reference_for(target) && q.tablet.is_none())
    {
        if let Some(category) = references.get(&identity(q)) {
            q.category = category.clone();
            if q.quality_at(Utc::now()) == Quality::Fresh && q.permits_annotation() {
                native.insert(identity(q));
            }
        }
    }
    quotes.retain(|q| !q.is_reference_for(target) || !native.contains(&identity(q)));
}
