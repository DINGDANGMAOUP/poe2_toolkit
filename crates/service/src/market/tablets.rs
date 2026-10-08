//! Independent tablet markets. Native currencies and rarity scopes stay separate.
use super::*;
use base64::Engine;
use poe2_core::{TabletIdentity, TabletRarity};
use std::{collections::BTreeSet, io::Read};
const BASE: &str = "http://125.122.32.215:2083";

fn server(scope: &MarketScope) -> &'static str {
    if scope.realm == Realm::China {
        "cn"
    } else {
        "international"
    }
}
fn rarity(text: &str) -> Result<TabletRarity> {
    Ok(match text {
        "normal" => TabletRarity::Normal,
        "magic" => TabletRarity::Magic,
        "rare" => TabletRarity::Rare,
        "unique" => TabletRarity::Unique,
        _ => bail!("未知碑牌稀有度"),
    })
}
fn stamp(value: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|x| x.with_timezone(&Utc))
}
fn resolve_league(
    data: &Value,
    scope: &MarketScope,
    international_name: Option<&str>,
) -> Result<String> {
    ensure!(data["server"] == server(scope), "碑牌目录服区不符");
    ensure!(data["stale"] != true, "碑牌赛季目录已过期");
    let rows = data["data"].as_array().context("碑牌目录格式变化")?;
    let matches: Vec<_> = rows
        .iter()
        .filter(|row| {
            row["hardcore"].as_bool() == Some(scope.hardcore)
                && if scope.realm == Realm::China {
                    row["source_season"].as_str() == Some(&scope.league)
                        || row["name"].as_str() == Some(&scope.league)
                        || row["slug"].as_str() == Some(&scope.league)
                } else {
                    row["name"].as_str() == international_name
                }
        })
        .collect();
    ensure!(
        matches.len() == 1,
        "碑牌目录没有唯一对应赛季；不切换到其他赛季"
    );
    required_text(matches[0], "name")
}

impl MarketClient {
    pub async fn refresh_tablets(
        &self,
        scope: &MarketScope,
        cancel: Arc<AtomicBool>,
    ) -> Result<MarketSnapshot> {
        check_cancel(&cancel)?;
        let international_name = if scope.realm == Realm::International {
            Some(
                self.leagues()
                    .await?
                    .into_iter()
                    .find(|l| l.id == scope.league && l.hardcore == scope.hardcore)
                    .context("所选国际服赛季失效")?
                    .name,
            )
        } else {
            None
        };
        let mut url = Url::parse(BASE)?.join("/api/v1/leagues")?;
        url.query_pairs_mut().extend_pairs([
            ("server", server(scope)),
            ("category", "tablet"),
            ("rarity", "rare"),
        ]);
        let league = resolve_league(&self.get(url).await?, scope, international_name.as_deref())?;
        let now = Utc::now();
        let mut quotes = Vec::new();
        let mut warnings = Vec::new();
        let mut revisions = Vec::new();
        for layer in ["names", "magic", "rare"] {
            check_cancel(&cancel)?;
            let result = if layer == "names" && scope.realm == Realm::International {
                self.ninja_tablet_names(&league, now).await
            } else {
                self.tablet_pages(scope, &league, layer, &cancel, BASE)
                    .await
            };
            match result {
                Ok((rows, revision, notes)) => {
                    quotes.extend(rows);
                    revisions.push(revision);
                    warnings.extend(notes);
                }
                Err(e) => warnings.push(format!(
                    "碑牌{}未更新：{e:#}",
                    match layer {
                        "names" => "名称",
                        "magic" => "魔法词缀",
                        _ => "稀有词缀",
                    }
                )),
            }
        }
        for quote in &mut quotes {
            quote.source_market = Some(scope.clone());
        }
        check_cancel(&cancel)?;
        ensure!(!quotes.is_empty(), "{}", warnings.join("；"));
        warnings.push(
            "碑牌为独立挂牌参考；魔法、稀有与原币种分别显示，词缀价不能相加估算整件。".into(),
        );
        let revision = digest(&serde_json::to_vec(&revisions)?);
        let id = digest(&serde_json::to_vec(&(scope, &quotes, &revision))?);
        Ok(MarketSnapshot {
            id,
            scope: scope.clone(),
            fetched_at: now,
            source_revision: Some(revision),
            quotes,
            warnings,
        })
    }
    async fn ninja_tablet_names(
        &self,
        league: &str,
        now: DateTime<Utc>,
    ) -> Result<(Vec<Quote>, String, Vec<String>)> {
        let mut quotes = Vec::new();
        let mut revisions = Vec::new();
        let mut warnings = Vec::new();
        for category in ["PrecursorTablets", "UniqueTablets"] {
            let mut url =
                Url::parse("https://poe.ninja/poe2/api/economy/stash/current/item/overview")?;
            url.query_pairs_mut()
                .extend_pairs([("league", league), ("type", category)]);
            match self.get(url).await.and_then(|data| {
                let rows = parse_ninja(&data, category, now)?;
                Ok((rows, digest(data.to_string().as_bytes())))
            }) {
                Ok((mut rows, revision)) => {
                    if category == "PrecursorTablets" {
                        for q in &mut rows {
                            q.category = "tablet:name".into();
                        }
                    }
                    quotes.extend(rows);
                    revisions.push(revision);
                }
                Err(e) => warnings.push(format!("{category} 未更新：{e:#}")),
            }
        }
        ensure!(!quotes.is_empty(), "{}", warnings.join("；"));
        Ok((quotes, digest(&serde_json::to_vec(&revisions)?), warnings))
    }
    async fn tablet_pages(
        &self,
        scope: &MarketScope,
        league: &str,
        layer: &str,
        cancel: &AtomicBool,
        base: &str,
    ) -> Result<(Vec<Quote>, String, Vec<String>)> {
        let whole = layer == "names";
        let mut offset = 0;
        let mut revision = None;
        let mut seen = BTreeSet::new();
        let mut quotes = Vec::new();
        let mut skipped = 0;
        loop {
            check_cancel(cancel)?;
            let mut url = Url::parse(base)?.join(if whole {
                "/api/v1/whole-tablets/prices"
            } else {
                "/api/v1/prices"
            })?;
            url.query_pairs_mut().extend_pairs([
                ("server", server(scope)),
                ("league", league),
                ("sort", "name"),
                ("limit", "200"),
                ("offset", &offset.to_string()),
            ]);
            if whole {
                url.query_pairs_mut().append_pair("market", "whole-tablet");
            } else {
                url.query_pairs_mut().extend_pairs([
                    ("category", "tablet"),
                    ("rarity", layer),
                    ("display_currency", "exalted"),
                ]);
                if scope.realm == Realm::China {
                    url.query_pairs_mut().append_pair("view", "live");
                }
            }
            let data = self.get(url).await?;
            let (current, total) = validate_page(&data, scope, league, layer, offset)?;
            if let Some(old) = &revision {
                ensure!(old == &current, "碑牌分页期间修订变化，拒绝混合快照");
            }
            revision = Some(current);
            let rows = data["data"].as_array().context("碑牌响应缺少 data")?;
            ensure!(
                !rows.is_empty() && rows.len() <= 200 && offset + rows.len() <= total,
                "碑牌分页不完整"
            );
            for row in rows {
                ensure!(seen.insert(required_text(row, "id")?), "碑牌分页身份重复");
                match parse_row(row, &data, scope, league, layer, Utc::now()) {
                    Ok(q) => quotes.push(q),
                    Err(_) => skipped += 1,
                }
            }
            offset += rows.len();
            if offset == total {
                break;
            }
        }
        ensure!(!quotes.is_empty(), "碑牌数据没有符合身份与价格契约的条目");
        let notes = if skipped > 0 {
            vec![format!(
                "碑牌 {layer}：{skipped} 条无报价或身份/查询条件不兼容，未纳入行情"
            )]
        } else {
            vec![]
        };
        Ok((quotes, revision.unwrap(), notes))
    }
}
fn validate_page(
    data: &Value,
    scope: &MarketScope,
    league: &str,
    layer: &str,
    offset: usize,
) -> Result<(String, usize)> {
    ensure!(
        data["server"] == server(scope) && data["league"] == league,
        "碑牌响应服区/赛季不符"
    );
    let whole = layer == "names";
    if whole {
        ensure!(data["market"] == "whole-tablet", "碑牌整件市场不符");
    } else {
        ensure!(
            data["category"] == "tablet" && data["rarity"] == layer,
            "碑牌词缀稀有度不符"
        );
    }
    for part in [
        &data["scope"],
        &data["snapshot"],
        &data["snapshot"]["config"],
        &data["partial_snapshot"],
    ] {
        for (key, expected) in [("server", server(scope)), ("league", league)] {
            ensure!(
                part[key].is_null() || part[key] == expected,
                "碑牌快照身份不符"
            );
        }
        if !whole {
            ensure!(
                part["rarity"].is_null() || part["rarity"] == layer,
                "碑牌快照稀有度不符"
            );
        }
    }
    let total = data["total"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= 12000)
        .context("碑牌分页总量无效")? as usize;
    ensure!(
        data["offset"].as_u64() == Some(offset as u64),
        "碑牌分页偏移不符"
    );
    ensure!(
        data["snapshot"]["id"].as_str().is_some()
            || data["partial_snapshot"]["id"].as_str().is_some(),
        "碑牌快照缺少修订身份"
    );
    let revision = digest(&serde_json::to_vec(&(
        total,
        &data["snapshot"],
        &data["partial_snapshot"],
        &data["updated_at"],
        &data["collection"]["run_id"],
    ))?);
    Ok((revision, total))
}
fn native_price(row: &Value) -> Result<(Decimal, String, u64)> {
    let mut candidates = Vec::new();
    let mut sum = 0;
    for (unit, sample) in row["prices"].as_object().context("缺少分币种报价")? {
        let count = sample["count"]
            .as_u64()
            .filter(|n| *n > 0 && *n <= 100000)
            .context("样本量无效")?;
        sum += count;
        if !matches!(unit.as_str(), "exalted" | "chaos" | "divine") {
            continue;
        }
        let low = decimal(&sample["min"])
            .filter(|v| *v > Decimal::ZERO)
            .context("最低价无效")?;
        let high = decimal(&sample["max"]).context("最高价无效")?;
        let median = decimal(&sample["low_sample_median"])
            .or_else(|| decimal(&sample["median"]))
            .context("缺少可验证的中位参考")?;
        ensure!(median >= low && median <= high, "原币种中位价超出样本范围");
        candidates.push((count, unit.clone(), median));
    }
    ensure!(
        row["sample_count"].as_u64() == Some(sum),
        "总样本数与币种样本不一致"
    );
    let (count, unit, amount) = candidates
        .into_iter()
        .max_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)))
        .context("无可用原币种报价")?;
    Ok((amount, unit, count))
}
fn decode_query(text: &str, scope: &MarketScope, league: &str) -> Result<Value> {
    ensure!(text.len() <= 100000, "查询过长");
    let url = Url::parse(text)?;
    let host = if scope.realm == Realm::China {
        "poe.game.qq.com"
    } else {
        "www.pathofexile.com"
    };
    ensure!(
        url.scheme() == "https"
            && url.host_str() == Some(host)
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "交易查询源不符"
    );
    let parts: Vec<_> = url.path_segments().context("缺少交易路径")?.collect();
    ensure!(
        parts.len() == 5 && parts[..3] == ["trade2", "search", "poe2"],
        "交易查询路径不符"
    );
    ensure!(
        percent_encoding::percent_decode_str(parts[3]).decode_utf8()? == league,
        "交易查询赛季不符"
    );
    let bytes =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[4].trim_end_matches('='))?;
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(bytes.as_slice())
        .take(65537)
        .read_to_end(&mut raw)?;
    ensure!(raw.len() <= 65536, "交易查询解压超限");
    Ok(serde_json::from_slice(&raw)?)
}
fn uses_stat(base: &str) -> Option<&'static str> {
    Some(match base {
        "Abyss_Tablet" => "2369421690",
        "Breach_Tablet" => "2219129443",
        "Delirium_Tablet" => "3879011313",
        "Expedition_Tablet" => "1714888636",
        "Irradiated_Tablet" => "4041853756",
        "Overseer_Tablet" => "3376302538",
        "Ritual_Tablet" => "3166002380",
        "Temple_Tablet" => "3035440454",
        _ => return None,
    })
}
fn validate_query(q: &Value, row: &Value, base: &str, rare: &str, whole: bool) -> Result<u64> {
    use serde_json::json;
    let expected_type = if whole {
        row["base_name"].as_str()
    } else if row["server"] == "cn" {
        row["tablet_name"].as_str()
    } else {
        row["tablet_en"].as_str()
    }
    .context("查询物品类型缺失")?;
    ensure!(
        q["type"] == expected_type && q["status"] == json!({"option":"securable"}),
        "查询底材/交易状态不符"
    );
    ensure!(
        q["filters"]
            == json!({"type_filters":{"filters":{"category":{"option":"map.tablet"},"rarity":{"option":rare}}},"trade_filters":{"filters":{"collapse":{"option":"false"}}}}),
        "交易筛选条件改变"
    );
    let unique = whole && rare == "unique";
    if unique {
        ensure!(q["name"] == row["name"], "传奇名称查询不符");
    }
    let keys = q.as_object().context("查询非对象")?;
    ensure!(
        keys.len() == if unique { 5 } else { 4 }
            && keys.keys().all(
                |k| matches!(k.as_str(), "status" | "type" | "stats" | "filters")
                    || unique && k == "name"
            ),
        "附加交易条件未适配"
    );
    let groups = q["stats"].as_array().context("查询缺少词缀条件")?;
    ensure!(
        groups.len() == if whole { 1 } else { 2 },
        "词缀条件数量未适配"
    );
    let id = format!("implicit.stat_{}", uses_stat(base).context("未知碑牌底材")?);
    let minimum = if whole
        && matches!(
            row["id"].as_str().unwrap_or(""),
            "Mastered_Domain"
                | "The_Grand_Project"
                | "Visions_of_Paradise"
                | "Unforeseen_Consequences"
                | "Overseer_Tablet:normal"
        ) {
        1
    } else if unique {
        5
    } else {
        10
    };
    let expected = json!({"type":"and","filters":[{"id":id,"value":{"min":minimum}}]});
    ensure!(
        groups.iter().filter(|g| **g == expected).count() == 1,
        "剩余次数与报价范围不符"
    );
    if !whole {
        let explicit = groups
            .iter()
            .find(|g| **g != expected)
            .context("缺少显式词缀")?;
        let filters = explicit["filters"].as_array().context("词缀筛选无效")?;
        ensure!(
            !filters.is_empty() && filters.len() <= 4,
            "词缀筛选范围过大"
        );
        ensure!(
            (explicit["type"] == "and"
                && filters.len() == 1
                && explicit.as_object().unwrap().len() == 2)
                || (explicit["type"] == "count"
                    && explicit["value"] == json!({"min":1})
                    && explicit.as_object().unwrap().len() == 3),
            "未适配的词缀组合"
        );
        for f in filters {
            ensure!(
                f["id"]
                    .as_str()
                    .is_some_and(
                        |s| s.strip_prefix("explicit.stat_").is_some_and(
                            |tail| !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit())
                        )
                    ),
                "非显式交易词缀"
            );
            ensure!(
                f.as_object().is_some_and(|m| m.len() == 1),
                "额外词缀区间条件未适配"
            );
        }
    }
    Ok(minimum)
}
fn parse_row(
    row: &Value,
    page: &Value,
    scope: &MarketScope,
    league: &str,
    layer: &str,
    now: DateTime<Utc>,
) -> Result<Quote> {
    let whole = layer == "names";
    ensure!(
        row["status"] == if whole { "priced" } else { "ok" },
        "来源暂无报价"
    );
    for (k, v) in [("server", server(scope)), ("league", league)] {
        ensure!(row[k].is_null() || row[k] == v, "行市场不符");
    }
    if !whole {
        ensure!(
            row["server"] == server(scope) && row["category"] == "tablet" && row["rarity"] == layer,
            "词缀行市场不符"
        );
    }
    let base = required_text(row, if whole { "base_id" } else { "tablet" })?;
    let rare = required_text(row, "rarity")?;
    let rarity = rarity(&rare)?;
    let query = decode_query(&required_text(row, "trade_url")?, scope, league)?;
    let minimum = validate_query(&query, row, &base, &rare, whole)?;
    let (amount, unit, count) = native_price(row)?;
    let id = required_text(row, "id")?;
    let (tablet, name, category) = if whole {
        let english = required_text(row, "name_en")?;
        ensure!(
            id == if rare == "unique" {
                english.replace(' ', "_")
            } else {
                format!("{base}:{rare}")
            },
            "整件身份不符"
        );
        if rare != "unique" {
            ensure!(english.replace(' ', "_") == base, "底材身份不符");
        }
        (
            TabletIdentity::Name {
                base,
                rarity: rarity.clone(),
                minimum_uses: minimum,
            },
            format!("{} · {}", required_text(row, "name")?, rarity.label()),
            "tablet:name",
        )
    } else {
        let generation = required_text(row, "generation")?;
        ensure!(
            matches!(generation.as_str(), "prefix" | "suffix"),
            "未知词缀生成类型"
        );
        (
            TabletIdentity::Affix {
                base,
                rarity: rarity.clone(),
                modifier: required_text(row, "name_en")?,
                generation,
                text_en: required_text(row, "text_en")?,
                query,
            },
            format!(
                "{} · {} · {}",
                required_text(row, "tablet_name")?,
                rarity.label(),
                required_text(row, "text")?
            ),
            "tablet:affix",
        )
    };
    let observed_at = stamp(&row["searched_at"]).or_else(|| stamp(&row["fetched_at"]));
    let stale = if whole {
        page["stale"] != false
    } else if !page["snapshot"].is_null() {
        page["snapshot"]["stale"] != false
    } else {
        stamp(&page["partial_snapshot"]["last_batch_at"])
            .is_none_or(|t| t > now || (now - t).num_seconds() > 14400)
    };
    let quality = if stale || observed_at.is_none_or(|t| t > now || (now - t).num_seconds() > 14400)
    {
        Quality::Stale
    } else {
        Quality::Fresh
    };
    Ok(Quote {
        variant: Default::default(),
        statistic: Default::default(),
        source_market: None,
        tablet: Some(tablet),
        item_id: format!("tablet:{layer}:{id}:{rare}"),
        name,
        category: category.into(),
        amount,
        unit,
        provider: "碑牌市场".into(),
        observed_at,
        fetched_at: now,
        sample_count: Some(count),
        quality,
        reasons: vec![
            format!("{league} · {} · 剩余使用次数至少 {minimum}", rarity.label()),
            "原币种低价挂牌样本中位参考；各稀有度独立，不与其他币种混算".into(),
            if whole {
                "整件报价与词缀价格分开；非成交保证".into()
            } else {
                "词缀查询参考，不写入物品名称".into()
            },
        ],
        metadata_id: None,
    })
}
