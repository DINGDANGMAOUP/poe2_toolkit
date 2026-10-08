use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Realm {
    #[default]
    International,
    China,
}

impl Realm {
    pub fn label(self) -> &'static str {
        match self {
            Self::International => "国际服",
            Self::China => "国服",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MarketScope {
    #[serde(default, skip_serializing_if = "crate::GameId::is_poe2")]
    pub game: crate::GameId,
    pub realm: Realm,
    pub league: String,
    pub hardcore: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct League {
    #[serde(default, skip_serializing_if = "crate::GameId::is_poe2")]
    pub game: crate::GameId,
    pub id: String,
    pub name: String,
    pub current: bool,
    pub hardcore: bool,
    pub realm: Realm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Fresh,
    Stale,
    LowEvidence,
    Conflicting,
    Missing,
}

impl Quality {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Fresh => "有效",
            Self::Stale => "已过期",
            Self::LowEvidence => "样本不足",
            Self::Conflicting => "存在冲突",
            Self::Missing => "缺失",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quote {
    #[serde(default, skip_serializing_if = "ItemVariant::is_empty")]
    pub variant: ItemVariant,
    #[serde(default)]
    pub statistic: PriceStatistic,
    /// Actual origin, independent from the market in which the quote is displayed.
    /// Older same-market snapshots did not store this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_market: Option<MarketScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tablet: Option<crate::TabletIdentity>,
    pub item_id: String,
    pub name: String,
    pub category: String,
    pub amount: Decimal,
    pub unit: String,
    pub provider: String,
    pub observed_at: Option<DateTime<Utc>>,
    pub fetched_at: DateTime<Utc>,
    pub sample_count: Option<u64>,
    pub quality: Quality,
    pub reasons: Vec<String>,
    /// Stable game identity, when explicitly provided by the source.
    pub metadata_id: Option<String>,
}

impl Quote {
    pub fn is_reference_for(&self, target: &MarketScope) -> bool {
        self.source_market
            .as_ref()
            .is_some_and(|source| source != target)
    }
    pub fn permits_market(&self, target: &MarketScope) -> bool {
        self.source_market.as_ref().is_none_or(|source| {
            source == target
                || (target.realm == Realm::China
                    && source.game == target.game
                    && source.realm == Realm::International
                    && source.hardcore == target.hardcore
                    && !source.league.trim().is_empty()
                    && self.feature() == crate::Feature::Unique)
        })
    }
    pub fn market_label(&self, target: &MarketScope) -> String {
        let source = self.source_market.as_ref().unwrap_or(target);
        format!(
            "{}{} · {} · {}",
            source.realm.label(),
            if self.is_reference_for(target) {
                "参考"
            } else {
                ""
            },
            source.league,
            if source.hardcore { "硬核" } else { "普通" }
        )
    }

    pub fn feature(&self) -> crate::Feature {
        match &self.tablet {
            Some(crate::TabletIdentity::Affix { .. }) => crate::Feature::TabletAffixes,
            Some(crate::TabletIdentity::Name { .. }) => crate::Feature::TabletNames,
            None if self.category == "unique:UniqueTablets" || self.category == "tablet:name" => {
                crate::Feature::TabletNames
            }
            None if self.category.starts_with("unique:") => crate::Feature::Unique,
            None if self.category == "cards" => crate::Feature::DivinationCards,
            None if self.category == "materials" => crate::Feature::Materials,
            None if self.category == "gems" => crate::Feature::Gems,
            None if self.category == "maps" => crate::Feature::Maps,
            _ => crate::Feature::Currency,
        }
    }
    pub fn quality_at(&self, now: DateTime<Utc>) -> Quality {
        if self.fetched_at > now
            || (now - self.fetched_at).num_seconds() > 4 * 3600
            || self
                .observed_at
                .is_some_and(|at| at > now || (now - at).num_seconds() > 4 * 3600)
        {
            Quality::Stale
        } else {
            self.quality.clone()
        }
    }
    pub fn display_price(&self) -> String {
        format!("{} {}", display_amount(self.amount, 2), self.unit)
    }

    pub fn permits_annotation(&self) -> bool {
        self.amount > Decimal::ZERO
            && self.quality == Quality::Fresh
            && self.variant.is_empty()
            && !matches!(self.feature(), crate::Feature::Gems | crate::Feature::Maps)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemVariant {
    pub links: Option<u32>,
    pub gem_level: Option<u32>,
    pub quality: Option<u32>,
    pub corrupted: Option<bool>,
    pub map_tier: Option<u32>,
    pub discriminator: Option<String>,
}
impl ItemVariant {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if let Some(n) = self.links {
            parts.push(format!("{n} 连"));
        }
        if let Some(n) = self.gem_level {
            parts.push(format!("{n} 级"));
        }
        if let Some(n) = self.quality {
            parts.push(format!("{n}% 品质"));
        }
        if let Some(n) = self.map_tier {
            parts.push(format!("T{n}"));
        }
        if let Some(v) = self.corrupted {
            parts.push(if v { "已腐化" } else { "未腐化" }.into());
        }
        if let Some(v) = &self.discriminator {
            parts.push(v.clone());
        }
        parts.join(" · ")
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriceStatistic {
    #[default]
    Unspecified,
    Exchange,
    Listing,
    DailyBuy,
    LatestBuy,
}
impl PriceStatistic {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unspecified => "来源参考价",
            Self::Exchange => "兑换市场参考",
            Self::Listing => "挂牌参考",
            Self::DailyBuy => "今日买一均价",
            Self::LatestBuy => "最新买一价",
        }
    }
}

/// Positive prices below display precision must not turn into a zero quote.
pub(crate) fn display_amount(amount: Decimal, decimals: u32) -> String {
    let rounded = amount.round_dp(decimals);
    if amount > Decimal::ZERO && rounded == Decimal::ZERO {
        format!("<{}", Decimal::new(1, decimals).normalize())
    } else {
        rounded.normalize().to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketSnapshot {
    pub id: String,
    pub scope: MarketScope,
    pub fetched_at: DateTime<Utc>,
    pub source_revision: Option<String>,
    pub quotes: Vec<Quote>,
    pub warnings: Vec<String>,
}

impl MarketSnapshot {
    pub fn age_seconds(&self, now: DateTime<Utc>) -> i64 {
        (now - self.fetched_at).num_seconds().max(0)
    }

    pub fn can_auto_apply(&self, scope: &MarketScope, now: DateTime<Utc>, max_age: i64) -> bool {
        &self.scope == scope
            && max_age > 0
            && self.fetched_at <= now
            && self.age_seconds(now) <= max_age
            && self
                .quotes
                .iter()
                .any(|q| q.permits_annotation() && q.permits_market(scope))
    }
}
