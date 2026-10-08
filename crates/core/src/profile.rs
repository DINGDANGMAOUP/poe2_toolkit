use crate::MarketScope;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    Currency,
    Unique,
    TabletNames,
    TabletAffixes,
    MapHints,
    DivinationCards,
    Materials,
    Gems,
    Maps,
}

impl Feature {
    pub const ALL: [Self; 5] = [
        Self::Currency,
        Self::Unique,
        Self::TabletNames,
        Self::TabletAffixes,
        Self::MapHints,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Currency => "通货价格",
            Self::Unique => "传奇价格",
            Self::TabletNames => "碑牌名称",
            Self::TabletAffixes => "碑牌词缀",
            Self::MapHints => "岛屿提示",
            Self::DivinationCards => "命运卡",
            Self::Materials => "材料价格",
            Self::Gems => "技能石",
            Self::Maps => "地图",
        }
    }
    pub fn for_game(game: crate::GameId) -> &'static [Self] {
        match game {
            crate::GameId::Poe1 => &[
                Self::Currency,
                Self::DivinationCards,
                Self::Materials,
                Self::Unique,
                Self::Gems,
                Self::Maps,
            ],
            crate::GameId::Poe2 => &Self::ALL,
        }
    }
    pub fn annotatable(self) -> bool {
        !matches!(self, Self::Gems | Self::Maps)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    Bundles,
    Ggpk,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Installation {
    #[serde(default, skip_serializing_if = "crate::GameId::is_poe2")]
    pub game: crate::GameId,
    pub id: String,
    pub root: PathBuf,
    pub storage: StorageKind,
    #[serde(default)]
    pub client_kind: ClientKind,
    #[serde(default)]
    pub client_realm: Option<crate::Realm>,
    #[serde(default)]
    pub client_evidence: Vec<String>,
    pub fingerprint: String,
    pub can_read: bool,
    pub can_write: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    Standalone,
    Steam,
    Epic,
    WeGame,
    #[default]
    Unknown,
}
impl ClientKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Standalone => "官方独立端",
            Self::Steam => "Steam",
            Self::Epic => "Epic",
            Self::WeGame => "WeGame",
            Self::Unknown => "渠道待确认",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default, skip_serializing_if = "crate::GameId::is_poe2")]
    pub game: crate::GameId,
    pub id: String,
    pub revision: u64,
    pub name: String,
    pub installation: Option<Installation>,
    pub market: Option<MarketScope>,
    pub features: Vec<Feature>,
    pub refresh_minutes: u32,
    pub auto_refresh: bool,
    pub auto_apply: bool,
    #[serde(default)]
    pub annotation: AnnotationPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnotationPolicy {
    pub allow_international_reference: bool,
    pub retention_minutes: u32,
    pub detailed: bool,
}
impl Default for AnnotationPolicy {
    fn default() -> Self {
        Self {
            allow_international_reference: false,
            retention_minutes: 240,
            detailed: false,
        }
    }
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            game: crate::GameId::Poe2,
            id: "default".into(),
            revision: 1,
            name: "默认方案".into(),
            installation: None,
            market: None,
            features: vec![Feature::Currency, Feature::Unique],
            refresh_minutes: 60,
            auto_refresh: false,
            auto_apply: false,
            annotation: AnnotationPolicy::default(),
        }
    }
}

impl Profile {
    /// Browsing another realm is allowed; price writes require a matching target.
    pub fn market_matches_client(&self) -> bool {
        self.installation
            .as_ref()
            .and_then(|i| i.client_realm)
            .zip(self.market.as_ref())
            .is_none_or(|(realm, market)| realm == market.realm)
    }
    pub fn for_game(game: crate::GameId) -> Self {
        Self {
            game,
            id: format!("{}-default", game.key()),
            name: format!("{} 默认方案", game.label()),
            ..Self::default()
        }
    }
    pub fn valid_context(&self) -> bool {
        self.market.as_ref().is_none_or(|s| s.game == self.game)
            && self
                .installation
                .as_ref()
                .is_none_or(|i| i.game == self.game)
            && self
                .features
                .iter()
                .all(|f| Feature::for_game(self.game).contains(f) && f.annotatable())
            && self.annotation.retention_minutes <= 1440
    }
}
