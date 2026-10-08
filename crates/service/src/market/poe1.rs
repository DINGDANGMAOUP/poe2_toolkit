//! PoE1's public economy contract. Item variants remain visible but never
//! become an unconditional name annotation.
use super::*;
use poe2_core::{GameId, ItemVariant, PriceStatistic};

const TYPES: &[(&str, &str, bool)] = &[
    ("Currency", "currency", true),
    ("Fragment", "currency", true),
    ("DivinationCard", "cards", true),
    ("Essence", "materials", true),
    ("Fossil", "materials", true),
    ("Resonator", "materials", true),
    ("Scarab", "materials", true),
    ("Oil", "materials", true),
    ("UniqueWeapon", "unique:weapon", false),
    ("UniqueArmour", "unique:armour", false),
    ("UniqueAccessory", "unique:accessory", false),
    ("UniqueFlask", "unique:flask", false),
    ("UniqueJewel", "unique:jewel", false),
    ("SkillGem", "gems", false),
    ("Map", "maps", false),
];
impl MarketClient {
    pub(super) async fn poe1_leagues(&self) -> Result<Vec<League>> {
        let data = self
            .get(Url::parse("https://poe.ninja/poe1/api/economy/leagues")?)
            .await?;
        let rows = data.as_array().context("PoE1 赛季目录结构不兼容")?;
        ensure!(
            !rows.is_empty() && rows.len() <= 100,
            "PoE1 赛季目录大小异常"
        );
        rows.iter()
            .map(|row| {
                let id = required_text(row, "id")?;
                let name = required_text(row, "name")?;
                Ok(League {
                    game: GameId::Poe1,
                    hardcore: name.starts_with("Hardcore") || name.starts_with("HC "),
                    current: !matches!(id.as_str(), "Standard" | "Hardcore"),
                    id,
                    name,
                    realm: Realm::International,
                })
            })
            .collect()
    }
    pub(super) async fn refresh_poe1_china(
        &self,
        target: &MarketScope,
        previous: Option<&MarketSnapshot>,
        cancel: Arc<AtomicBool>,
    ) -> Result<MarketSnapshot> {
        let mut snapshot = self
            .refresh_china(target, cancel.clone(), china::BASE)
            .await?;
        let references = async {
            let domestic = self.leagues_for_game(GameId::Poe1, Realm::China).await?;
            let international = self.poe1_leagues().await?;
            let selected = domestic
                .iter()
                .find(|l| l.id == target.league)
                .context("国服赛季已改变")?;
            let source = international
                .iter()
                .find(|l| {
                    l.id.eq_ignore_ascii_case(&target.league) && l.hardcore == target.hardcore
                })
                .or_else(|| {
                    selected
                        .current
                        .then(|| {
                            international
                                .iter()
                                .find(|l| l.current && l.hardcore == target.hardcore)
                        })
                        .flatten()
                })
                .context("此历史赛季没有可确认的国际参考市场")?;
            let scope = MarketScope {
                game: GameId::Poe1,
                realm: Realm::International,
                league: source.id.clone(),
                hardcore: source.hardcore,
            };
            let previous = previous
                .filter(|s| &s.scope == target)
                .map(|s| MarketSnapshot {
                    scope: scope.clone(),
                    quotes: s
                        .quotes
                        .iter()
                        .filter(|q| q.source_market.as_ref() == Some(&scope))
                        .cloned()
                        .collect(),
                    ..s.clone()
                });
            let result = self
                .refresh_poe1_types(&scope, previous.as_ref(), cancel.clone(), true)
                .await?;
            let after = china::parse_leagues_for(
                GameId::Poe1,
                &self
                    .get_checked(china::seasons_url_for(china::BASE, GameId::Poe1)?, true)
                    .await?,
            )?;
            ensure!(
                after.first().map(|l| &l.id) == domestic.first().map(|l| &l.id),
                "采价期间国服赛季改变，国际参考未激活"
            );
            Ok::<_, anyhow::Error>(result)
        }
        .await;
        match references {
            Ok(reference) => {
                snapshot.warnings.push(format!(
                    "传奇含 {} 国际参考，保持原币种；默认不写入，需在方案中允许国际参考",
                    reference.scope.league
                ));
                snapshot.warnings.extend(reference.warnings);
                snapshot.quotes.extend(reference.quotes);
                super::uniques::prefer_domestic(&mut snapshot.quotes, target);
            }
            Err(error) => snapshot
                .warnings
                .push(format!("国际传奇参考未更新：{error:#}")),
        }
        check_cancel(&cancel)?;
        snapshot.id = digest(&serde_json::to_vec(&(
            &snapshot.scope,
            &snapshot.quotes,
            snapshot.fetched_at,
        ))?);
        Ok(snapshot)
    }
    pub(super) async fn refresh_poe1(
        &self,
        scope: &MarketScope,
        previous: Option<&MarketSnapshot>,
        cancel: Arc<AtomicBool>,
    ) -> Result<MarketSnapshot> {
        self.refresh_poe1_types(scope, previous, cancel, false)
            .await
    }
    async fn refresh_poe1_types(
        &self,
        scope: &MarketScope,
        previous: Option<&MarketSnapshot>,
        cancel: Arc<AtomicBool>,
        uniques_only: bool,
    ) -> Result<MarketSnapshot> {
        let leagues = self.poe1_leagues().await?;
        ensure!(
            leagues
                .iter()
                .any(|l| l.id == scope.league && l.hardcore == scope.hardcore),
            "PoE1 赛季或模式已失效，请重新选择"
        );
        let mut quotes = Vec::new();
        let mut warnings = Vec::new();
        let mut success = 0;
        let types: Vec<_> = TYPES
            .iter()
            .copied()
            .filter(|(_, category, _)| !uniques_only || category.starts_with("unique:"))
            .collect();
        for batch in types.chunks(3) {
            check_cancel(&cancel)?;
            let mut tasks = tokio::task::JoinSet::new();
            for &(kind, category, exchange) in batch {
                let client = self.clone();
                let scope = scope.clone();
                tasks.spawn(async move {
                    let result = async {
                        let path = if exchange {
                            "exchange/current/overview"
                        } else {
                            "stash/current/item/overview"
                        };
                        let mut url =
                            Url::parse(&format!("https://poe.ninja/poe1/api/economy/{path}"))?;
                        url.query_pairs_mut()
                            .extend_pairs([("league", scope.league.as_str()), ("type", kind)]);
                        parse(
                            &client.get(url).await?,
                            &scope,
                            kind,
                            category,
                            exchange,
                            Utc::now(),
                        )
                    }
                    .await;
                    (kind, category, result)
                });
            }
            while let Some(result) = tasks.join_next().await {
                let (kind, category, result) = result?;
                match result {
                    Ok(rows) => {
                        success += 1;
                        quotes.extend(rows);
                    }
                    Err(error) => {
                        warnings.push(format!("{kind} 未更新：{error:#}"));
                        // Cache ages stay unchanged. A failed request cannot rejuvenate a quote.
                        if let Some(old) = previous.filter(|s| &s.scope == scope) {
                            quotes.extend(
                                old.quotes
                                    .iter()
                                    .filter(|q| {
                                        q.category == category
                                            && q.item_id.starts_with(&format!("ninja:poe1:{kind}:"))
                                    })
                                    .cloned(),
                            );
                        }
                    }
                }
            }
        }
        check_cancel(&cancel)?;
        ensure!(success > 0, "PoE1 行情源全部失败：{}", warnings.join("；"));
        quotes.sort_by(|a, b| a.item_id.cmp(&b.item_id));
        let fetched_at = Utc::now();
        let source_revision = Some(digest(&serde_json::to_vec(&quotes)?));
        Ok(MarketSnapshot {
            id: digest(&serde_json::to_vec(&(scope, &source_revision, fetched_at))?),
            scope: scope.clone(),
            fetched_at,
            source_revision,
            quotes,
            warnings,
        })
    }
}
fn parse(
    data: &Value,
    scope: &MarketScope,
    kind: &str,
    category: &str,
    exchange: bool,
    now: DateTime<Utc>,
) -> Result<Vec<Quote>> {
    let rows = data["lines"].as_array().context("poe.ninja 缺少报价列表")?;
    ensure!(rows.len() <= 100_000, "报价数量超限");
    let items = data["items"].as_array();
    let core_items = data["core"]["items"].as_array();
    let primary = data["core"]["primary"].as_str().unwrap_or("chaos");
    let mut quotes = Vec::new();
    for row in rows {
        let Some(amount) = decimal(
            &row[if exchange {
                "primaryValue"
            } else {
                "chaosValue"
            }],
        )
        .filter(|x| *x > Decimal::ZERO) else {
            continue;
        };
        let id = row["id"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| row["id"].as_u64().map(|n| n.to_string()))
            .context("报价身份缺失")?;
        let metadata = items
            .into_iter()
            .flatten()
            .chain(core_items.into_iter().flatten())
            .find(|i| i["id"] == row["id"]);
        let name = row["name"]
            .as_str()
            .or_else(|| metadata.and_then(|v| v["name"].as_str()))
            .context("报价名称缺失")?
            .to_owned();
        let variant = ItemVariant {
            links: row["links"]
                .as_u64()
                .filter(|v| *v > 0)
                .and_then(|v| v.try_into().ok()),
            gem_level: row["gemLevel"].as_u64().and_then(|v| v.try_into().ok()),
            quality: row["gemQuality"].as_u64().and_then(|v| v.try_into().ok()),
            corrupted: row["corrupted"].as_bool().filter(|v| *v),
            map_tier: row["mapTier"].as_u64().and_then(|v| v.try_into().ok()),
            discriminator: row["variant"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
        };
        let count = row["listingCount"]
            .as_u64()
            .or_else(|| row["count"].as_u64());
        let quality = if exchange || count.is_some_and(|n| n > 0) {
            Quality::Fresh
        } else {
            Quality::LowEvidence
        };
        let item_id = format!(
            "ninja:poe1:{kind}:{}",
            digest(&serde_json::to_vec(&(&id, &variant))?)
        );
        quotes.push(Quote {
            variant,
            statistic: if exchange {
                PriceStatistic::Exchange
            } else {
                PriceStatistic::Listing
            },
            source_market: Some(scope.clone()),
            tablet: None,
            item_id,
            name,
            category: category.into(),
            amount,
            unit: if exchange {
                primary.into()
            } else {
                "chaos".into()
            },
            provider: "poe.ninja".into(),
            observed_at: None,
            fetched_at: now,
            sample_count: count,
            quality,
            reasons: vec!["来源未提供逐项采样时间；显示获取时间，复杂变体仅供行情参考".into()],
            metadata_id: None,
        });
    }
    Ok(quotes)
}
