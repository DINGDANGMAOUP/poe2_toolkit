use chrono::{DateTime, Local, Utc};
use poe2_core::OperationState;
pub fn timestamp(value: DateTime<Utc>) -> String {
    value
        .with_timezone(&Local)
        .format("%m-%d %H:%M")
        .to_string()
}
pub fn state_label(state: &OperationState) -> &'static str {
    match state {
        OperationState::Preparing => "准备中",
        OperationState::Prepared => "已准备",
        OperationState::Writing => "写入中",
        OperationState::Verifying => "校验中",
        OperationState::Committed => "写入已校验",
        OperationState::RolledBack => "已还原",
        OperationState::FailedUnchanged => "未修改",
        OperationState::RecoveryRequired => "待恢复",
    }
}
pub fn category_label(category: &str) -> &str {
    if let Some(label) = category.strip_prefix("cn:") {
        return label;
    }
    match category {
        "cards" => "命运卡",
        "materials" => "材料",
        "gems" => "技能石",
        "maps" => "地图",
        "tablet:name" | "unique:UniqueTablets" => "碑牌名称",
        "tablet:affix" => "碑牌词缀",
        "currency" => "通货",
        "abyss" => "深渊",
        "runes" => "符文",
        "fragments" => "碎片",
        "essences" => "精华",
        "ultimatum" => "最后通牒",
        "expedition" => "探险",
        "ritual" => "祭祀",
        "delirium" => "惊悸迷雾",
        "breach" => "裂隙",
        "unique:weapon" | "unique:UniqueWeapon" | "unique:UniqueWeapons" => "传奇武器",
        "unique:armour" | "unique:UniqueArmour" | "unique:UniqueArmours" => "传奇护甲",
        "unique:accessory" | "unique:UniqueAccessory" | "unique:UniqueAccessories" => "传奇饰品",
        "unique:jewel" | "unique:UniqueJewel" | "unique:UniqueJewels" => "传奇珠宝",
        "unique:flask" | "unique:UniqueFlask" | "unique:UniqueFlasks" => "传奇药剂",
        "unique:UniqueMap" => "传奇地图",
        "unique:UniqueRelic" | "unique:UniqueSanctumRelics" => "传奇圣物",
        "unique:UniqueCharm" | "unique:UniqueCharms" => "传奇护符",
        _ => category,
    }
}
