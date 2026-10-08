//! POE Currency's public POE2 market. No international fallback or implicit FX.
use super::*;
use chrono::{FixedOffset, NaiveDateTime, TimeZone};
use std::collections::BTreeSet;

pub(super) const BASE: &str = "https://poecurrency.top";

pub(super) fn seasons_url_for(base: &str, game: GameId) -> Result<Url> {
    let mut url = Url::parse(base)?.join("/api/season_list")?;
    url.query_pairs_mut().append_pair("version", game.key());
    Ok(url)
}

pub(super) fn parse_leagues_for(game: GameId, data: &Value) -> Result<Vec<League>> {
    let rows = data.as_array().context("国服赛季目录结构不兼容")?;
    ensure!(
        !rows.is_empty() && rows.len() <= 100,
        "国服赛季目录为空或超出上限"
    );
    let mut ids = BTreeSet::new();
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let id = row
                .as_str()
                .filter(|s| {
                    !s.trim().is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)
                })
                .context("国服赛季目录包含无效身份")?;
            ensure!(ids.insert(id), "国服赛季身份重复");
            Ok(League {
                game,
                id: id.into(),
                name: id.into(),
                current: index == 0,
                hardcore: false,
                realm: Realm::China,
            })
        })
        .collect()
}

fn selected<'a>(leagues: &'a [League], scope: &MarketScope) -> Result<&'a League> {
    ensure!(
        scope.realm == Realm::China && !scope.hardcore,
        "国服行情源仅提供普通模式，不能用于硬核市场"
    );
    leagues
        .iter()
        .find(|l| l.game == scope.game && l.id == scope.league && l.realm == scope.realm)
        .context("国服赛季身份已失效，请重新获取并选择赛季")
}

fn summary_url(base: &str, league: &League) -> Result<Url> {
    let mut url = Url::parse(base)?.join("/api/summary")?;
    url.query_pairs_mut()
        .append_pair("version", league.game.version());
    // The provider's current data is unqualified. Explicit season means archive.
    if !league.current {
        url.query_pairs_mut().append_pair("season", &league.id);
    }
    Ok(url)
}

impl MarketClient {
    pub(super) async fn refresh_china(
        &self,
        scope: &MarketScope,
        cancel: Arc<AtomicBool>,
        base: &str,
    ) -> Result<MarketSnapshot> {
        check_cancel(&cancel)?;
        ensure!(
            scope.realm == Realm::China && !scope.hardcore,
            "国服行情源不支持硬核市场"
        );
        let before = parse_leagues_for(
            scope.game,
            &self
                .get_checked(seasons_url_for(base, scope.game)?, true)
                .await?,
        )?;
        let league = selected(&before, scope)?;
        check_cancel(&cancel)?;
        let data = self.get(summary_url(base, league)?).await?;
        check_cancel(&cancel)?;
        let after = parse_leagues_for(
            scope.game,
            &self
                .get_checked(seasons_url_for(base, scope.game)?, true)
                .await?,
        )?;
        let confirmed = selected(&after, scope)?;
        ensure!(
            before[0].id == after[0].id && league.current == confirmed.current,
            "国服赛季在采价期间发生切换，本次数据未保存，请重新获取赛季"
        );
        check_cancel(&cancel)?;
        parse_summary(scope.clone(), &data, Utc::now())
    }
}

fn source_time(value: &Value) -> Option<DateTime<Utc>> {
    let text = value.as_str()?;
    if let Ok(time) = DateTime::parse_from_rfc3339(text) {
        return Some(time.with_timezone(&Utc));
    }
    ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"]
        .into_iter()
        .find_map(|format| NaiveDateTime::parse_from_str(text, format).ok())
        .and_then(|time| {
            FixedOffset::east_opt(8 * 3600)?
                .from_local_datetime(&time)
                .single()
        })
        .map(|time| time.with_timezone(&Utc))
}

fn positive(row: &Value, field: &str) -> Option<Decimal> {
    decimal(&row[field]).filter(|value| *value > Decimal::ZERO)
}

fn wide_spread(left: Decimal, right: Decimal) -> bool {
    // Division avoids overflow when a source returns an extreme numeric value.
    left.max(right)
        .checked_div(left.min(right))
        .is_none_or(|ratio| ratio > Decimal::from(3))
}

pub(super) fn parse_summary(
    scope: MarketScope,
    data: &Value,
    now: DateTime<Utc>,
) -> Result<MarketSnapshot> {
    ensure!(
        scope.realm == Realm::China && !scope.hardcore,
        "国服数据不能归入其他市场"
    );
    let categories = data.as_array().context("国服行情结构不兼容")?;
    ensure!(categories.len() <= 1000, "国服行情分类过多");
    let mut selected: BTreeMap<String, Quote> = BTreeMap::new();
    let mut total = 0usize;
    let mut skipped = 0usize;
    for category in categories {
        let label = required_text(category, "category_label")?;
        let rows = category["items"]
            .as_array()
            .context("国服分类缺少物品数组")?;
        total += rows.len();
        ensure!(total <= 100_000, "国服报价数量超过上限");
        for row in rows {
            let name = required_text(row, "item_name")?;
            let identity = row["engname"]
                .as_str()
                .filter(|s| !s.trim().is_empty() && s.len() < 2048)
                .unwrap_or(&name)
                .trim()
                .to_lowercase();
            // A tuple prevents names/category separators from aliasing identities.
            let item_id = format!(
                "poecurrency:{}",
                serde_json::to_string(&(&label, &identity))?
            );
            let latest_buy = positive(row, "latest_buy1");
            let latest_sell = positive(row, "latest_sell1");
            let choice = [
                ("latest_buy1", "最新买一参考"),
                ("latest_sell1", "最新卖一参考"),
                ("buy_avg", "今日买一均价"),
                ("sell_avg", "今日卖出均价"),
            ]
            .into_iter()
            .find_map(|(field, description)| {
                positive(row, field).map(|price| (price, field, description))
            });
            let Some((amount, field, description)) = choice else {
                skipped += 1;
                continue;
            };
            let raw_unit = row["currency_unit"]
                .as_str()
                .unwrap_or("")
                .trim()
                .to_lowercase();
            let unit = match raw_unit.as_str() {
                "e" => "exalted",
                "d" => "divine",
                "c" => "chaos",
                _ => "unknown",
            };
            let observed_at = source_time(&row["latest_datetime"]);
            let mut reasons = vec![format!(
                "国服通货市场 OCR · {description}；非成交保证，来源未提供样本数"
            )];
            if let Some(english) = row["engname"]
                .as_str()
                .filter(|s| !s.trim().is_empty() && s.len() < 2048)
            {
                reasons.push(format!("英文名：{english}"));
            }
            let mut quality = Quality::Fresh;
            if unit == "unknown" {
                reasons.push("计价单位缺失或未知，未猜测币种".into());
                quality = Quality::LowEvidence;
            }
            if latest_buy.is_none()
                || latest_sell.is_none()
                || matches!(field, "buy_avg" | "sell_avg")
            {
                reasons.push("缺少完整的最新双边报价；仅供参考".into());
                quality = Quality::LowEvidence;
            }
            if row["error"].as_bool() != Some(false)
                || row["anomaly_count"].as_u64().unwrap_or(0) > 0
            {
                reasons.push("来源报告异常或未提供明确的异常状态；不用于价格标注".into());
                if let Some(detail) = row["error_info"].as_str().filter(|s| !s.is_empty()) {
                    reasons.push(detail.chars().take(512).collect());
                }
                quality = Quality::Conflicting;
            }
            if latest_buy
                .zip(latest_sell)
                .is_some_and(|(buy, sell)| wide_spread(buy, sell))
            {
                reasons.push("最新买卖价差超过三倍，保留原值并标记冲突".into());
                quality = Quality::Conflicting;
            }
            if observed_at.is_none_or(|time| time > now || (now - time).num_seconds() > 4 * 3600) {
                reasons.push("来源采集时间缺失、异常或超过四小时".into());
                quality = Quality::Stale;
            }
            let mut quote = Quote {
                variant: Default::default(),
                statistic: if field == "latest_buy1" {
                    poe2_core::PriceStatistic::LatestBuy
                } else if field == "buy_avg" {
                    poe2_core::PriceStatistic::DailyBuy
                } else {
                    poe2_core::PriceStatistic::Unspecified
                },
                source_market: Some(scope.clone()),
                item_id: item_id.clone(),
                name,
                category: if scope.game == GameId::Poe1 && label.contains("命运卡") {
                    "cards".into()
                } else if scope.game == GameId::Poe1
                    && ["精华", "化石", "圣甲虫", "共振器", "圣油"]
                        .iter()
                        .any(|s| label.contains(s))
                {
                    "materials".into()
                } else {
                    format!("cn:{label}")
                },
                amount,
                unit: unit.into(),
                provider: "poecurrency.top".into(),
                observed_at,
                fetched_at: now,
                sample_count: None,
                quality,
                reasons,
                metadata_id: None,
                tablet: None,
            };
            if let Some(old) = selected.get_mut(&item_id) {
                if old.amount != quote.amount
                    || old.unit != quote.unit
                    || old.quality == Quality::Conflicting
                    || quote.quality == Quality::Conflicting
                {
                    let warning = "同一物品出现不同报价或币种，存在冲突".to_owned();
                    old.quality = Quality::Conflicting;
                    old.reasons.push(warning.clone());
                    quote.quality = Quality::Conflicting;
                    quote.reasons.push(warning);
                }
                if old.observed_at >= quote.observed_at {
                    continue;
                }
            }
            selected.insert(item_id, quote);
        }
    }
    ensure!(
        !selected.is_empty(),
        "该国服赛季暂无可用行情（归档可能尚未发布），保留原有缓存"
    );
    let mut warnings = vec![if scope.game == poe2_core::GameId::Poe1 {
        "国服通货与材料来源：poecurrency.top；报价保留原币种与统计口径。".into()
    } else {
        "国服通货与材料来源：poecurrency.top；碑牌使用独立来源，不混入国际服价格。".into()
    }];
    if skipped > 0 {
        warnings.push(format!("{skipped} 项没有正数报价，未纳入行情"));
    }
    let mut snapshot = MarketSnapshot {
        id: String::new(),
        scope,
        fetched_at: now,
        source_revision: Some(format!(
            "poecurrency-v1:{}",
            digest(&serde_json::to_vec(data)?)
        )),
        quotes: selected.into_values().collect(),
        warnings,
    };
    snapshot
        .quotes
        .sort_by(|a, b| a.category.cmp(&b.category).then(a.name.cmp(&b.name)));
    snapshot.id = digest(&serde_json::to_vec(&(
        &snapshot.scope,
        &snapshot.quotes,
        &snapshot.source_revision,
    ))?);
    Ok(snapshot)
}
