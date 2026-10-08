use serde::{Deserialize, Serialize};

/// Every market, installation and write plan belongs to exactly one game.
/// Legacy records produced by this application are PoE2 records.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum GameId {
    Poe1,
    #[default]
    Poe2,
}

impl GameId {
    pub const ALL: [Self; 2] = [Self::Poe1, Self::Poe2];
    pub fn key(self) -> &'static str {
        match self {
            Self::Poe1 => "poe1",
            Self::Poe2 => "poe2",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Poe1 => "PoE1",
            Self::Poe2 => "PoE2",
        }
    }
    pub fn version(self) -> &'static str {
        match self {
            Self::Poe1 => "1",
            Self::Poe2 => "2",
        }
    }
    pub fn is_poe2(&self) -> bool {
        *self == Self::Poe2
    }
}
