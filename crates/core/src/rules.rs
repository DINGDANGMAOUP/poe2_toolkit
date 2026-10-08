use serde::{Deserialize, Serialize};

/// Deliberately declarative: a rule update cannot execute code or select paths.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnnotationRules {
    #[serde(default = "crate::builtin_islands")]
    pub islands: Vec<crate::IslandHint>,
    pub price_decimals: u32,
    pub max_age_minutes: u32,
    pub minimum_listing_count: u64,
}
impl Default for AnnotationRules {
    fn default() -> Self {
        Self {
            islands: crate::builtin_islands(),
            price_decimals: 2,
            max_age_minutes: 240,
            minimum_listing_count: 1,
        }
    }
}
impl AnnotationRules {
    pub fn valid(&self) -> bool {
        self.islands.len() <= 128
            && self.islands.iter().all(|r| {
                r.id.starts_with("island:")
                    && r.id.len() <= 160
                    && !r.map_name.is_empty()
                    && r.map_name.len() <= 160
                    && r.detail.len() <= 256
                    && !r
                        .id
                        .chars()
                        .chain(r.map_name.chars())
                        .chain(r.detail.chars())
                        .any(char::is_control)
                    && r.priority
                        .as_ref()
                        .is_none_or(|p| matches!(p.as_str(), "S" | "A" | "B" | "C"))
            })
            && self
                .islands
                .iter()
                .map(|r| &r.id)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == self.islands.len()
            && self.price_decimals <= 4
            && (15..=240).contains(&self.max_age_minutes)
            && (1..=10000).contains(&self.minimum_listing_count)
    }
    pub fn permits(&self, quote: &crate::Quote, now: chrono::DateTime<chrono::Utc>) -> bool {
        let age = (now - quote.fetched_at).num_seconds();
        self.valid()
            && quote.permits_annotation()
            && (0..=i64::from(self.max_age_minutes) * 60).contains(&age)
            && quote.observed_at.is_none_or(|at| {
                (0..=i64::from(self.max_age_minutes) * 60).contains(&(now - at).num_seconds())
            })
            && quote
                .sample_count
                .is_none_or(|n| n >= self.minimum_listing_count)
    }
    pub fn price(&self, quote: &crate::Quote) -> String {
        format!(
            "{} {}",
            crate::market::display_amount(quote.amount, self.price_decimals),
            quote.unit
        )
    }
}
