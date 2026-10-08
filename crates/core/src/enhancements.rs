//! Typed identities for references; prices are never inferred from display text.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TabletRarity {
    Normal,
    Magic,
    Rare,
    Unique,
}
impl TabletRarity {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Normal => "普通",
            Self::Magic => "魔法",
            Self::Rare => "稀有",
            Self::Unique => "传奇",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TabletIdentity {
    Name {
        base: String,
        rarity: TabletRarity,
        minimum_uses: u64,
    },
    Affix {
        base: String,
        rarity: TabletRarity,
        modifier: String,
        generation: String,
        text_en: String,
        query: serde_json::Value,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IslandHint {
    /// Catalogue identity, not a guessed DAT row number.
    pub id: String,
    pub map_name: String,
    pub detail: String,
    pub priority: Option<String>,
}
impl IslandHint {
    pub fn text(&self) -> String {
        let rank = self
            .priority
            .as_ref()
            .map(|x| format!("{x} · "))
            .unwrap_or_default();
        let detail = if self.detail.is_empty() {
            String::new()
        } else {
            format!(" · {}", self.detail)
        };
        format!("{rank}{}{detail}", self.map_name)
    }
}
pub fn builtin_islands() -> Vec<IslandHint> {
    // Map/boss facts from the published reference catalogue; subjective grades
    // are deliberately left unset. Signed rules may set explicit priorities.
    [
        ("Castaway", "颠沛领域", "金币"),
        ("Untainted Paradise", "纯净乐园", "经验"),
        ("The Fractured Lake", "千裂泽", "独特基底装备"),
        ("Moment of Zen", "顿悟时刻", "传奇装备"),
        ("The Jade Isles", "青玉群岛", "首领战"),
        ("Barren Atoll", "贫瘠环礁", ""),
        ("Sprawling Jungle", "蔓生丛林", "梅德维德"),
        ("Mournful Cliffside", "恸哭悬崖", "沃拉娜"),
        ("Secluded Temple", "静谧神庙", "乌特雷"),
        ("Obscure Island", "无名之岛", "奥尔罗斯"),
        ("Stagnant Basin", "死水盆地", ""),
        ("Exhumed Ruins", "掘尸遗迹", ""),
        ("Sloughed Gully", "脱落沟壑", ""),
        ("Moor of Fallen Skies", "天陨荒原", "八孔遗物"),
        ("Craggy Peninsula", "乱石半岛", ""),
        ("Grazed Prairie", "牧野荒原", ""),
        ("Bleached Shoals", "褪色浅滩", ""),
        ("Lush Isle", "笼葱海岛", ""),
        ("Frigid Bluffs", "凛风悬崖", ""),
        ("Scorched Cay", "焦灼小岛", ""),
    ]
    .into_iter()
    .map(|(id, name, detail)| IslandHint {
        id: format!("island:{id}"),
        map_name: name.into(),
        detail: detail.into(),
        priority: None,
    })
    .collect()
}
