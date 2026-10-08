//! Read-only client identity probing, separate from schema/write capability checks.
use anyhow::{Context, Result, ensure};
use poe2_core::{ClientKind, GameId, Realm, StorageKind};
mod version;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize)]
pub struct ClientIdentity {
    pub root: PathBuf,
    pub game: GameId,
    pub storage: StorageKind,
    pub client_kind: ClientKind,
    pub client_realm: Option<Realm>,
    pub client_evidence: Vec<String>,
    pub resource_game_verified: bool,
}

/// Launcher records are discovery evidence, never resource-write authorization.
#[derive(Debug, Clone, Default)]
pub struct ClientHint {
    pub game: Option<GameId>,
    pub channel: ClientKind,
    pub source: String,
}

pub fn game_from_name(name: &str) -> Option<GameId> {
    let name = name
        .to_lowercase()
        .replace([' ', '_', '-', '(', ')', '（', '）'], "");
    if name.contains("pathofexile2")
        || name.contains("pathofexileii")
        || name.contains("流放之路2")
        || name.contains("流放之路：降临")
        || name.contains("流放之路:降临")
        || name == "poe2"
    {
        Some(GameId::Poe2)
    } else if name == "pathofexile" || name == "流放之路" || name == "poe1" {
        Some(GameId::Poe1)
    } else {
        None
    }
}

pub fn game_from_major(major: u32) -> Option<GameId> {
    match major {
        3 => Some(GameId::Poe1),
        4.. => Some(GameId::Poe2),
        _ => None,
    }
}

fn has_resources(root: &Path) -> bool {
    root.join("Bundles2/_.index.bin").is_file()
        || root.join("_.index.bin").is_file()
        || root.join("Content.ggpk").is_file()
        || root.join("content.ggpk").is_file()
}

/// Accept a root, Bundles2 directory, executable, index or GGPK; never scan a disk.
pub fn normalize_root(path: &Path) -> Result<PathBuf> {
    let original = std::fs::canonicalize(path).context("所选路径不存在或无法访问")?;
    let directory = if original.is_file() {
        original.parent().context("文件缺少父目录")?.to_path_buf()
    } else {
        original
    };
    for candidate in directory.ancestors().take(4) {
        if candidate
            .file_name()
            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("Bundles2"))
            && candidate.join("_.index.bin").is_file()
        {
            return Ok(candidate
                .parent()
                .context("Bundles2 缺少安装根目录")?
                .to_path_buf());
        }
        if has_resources(candidate) {
            return Ok(candidate.to_path_buf());
        }
    }
    Ok(directory)
}

fn distribution(
    root: &Path,
    storage: &StorageKind,
    hints: &[ClientHint],
) -> (ClientKind, Option<Realm>, Vec<String>) {
    let entries = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .take(4096)
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().to_lowercase())
        .collect::<std::collections::BTreeSet<_>>();
    let mut evidence = Vec::new();
    let mut china = Vec::new();
    for name in [
        "wegame.ini",
        "rail_api64.dll",
        "rail_files",
        "wegamelauncher",
        "tcls",
        "anticheatexpert",
        "qqopensdk.dll",
    ] {
        if entries.contains(name) {
            china.push(name.to_owned());
        }
    }
    // The reference counts the MSDK family once, irrespective of DLL count.
    if entries
        .iter()
        .any(|s| s.starts_with("msdk") && s.ends_with(".dll"))
    {
        china.push("MSDK*.dll".into());
    }
    let mut channels = Vec::new();
    if china.len() >= 2 {
        channels.push(ClientKind::WeGame);
        evidence.push(format!("国服特征（至少两项）：{}", china.join("、")));
    } else if !china.is_empty() {
        evidence.push(format!(
            "仅发现一项国服特征 {}，不足以单独判定国服",
            china[0]
        ));
    }
    if [
        "steam_api64.dll",
        "steam_api.dll",
        "pathofexilesteam.exe",
        "pathofexile_x64steam.exe",
        "pathofexilesteam2.exe",
        "pathofexile2steam.exe",
        "pathofexile2_x64steam.exe",
    ]
    .iter()
    .any(|name| entries.contains(*name))
    {
        channels.push(ClientKind::Steam);
        evidence.push("Steam 程序或 API 文件".into());
    }
    if entries.contains(".egstore") {
        channels.push(ClientKind::Epic);
        evidence.push("Epic .egstore 安装标记".into());
    }
    for hint in hints {
        if hint.channel != ClientKind::Unknown {
            channels.push(hint.channel);
            evidence.push(format!("{}：{}", hint.source, hint.channel.label()));
        }
    }
    if channels.is_empty() && china.is_empty() && *storage == StorageKind::Ggpk {
        channels.push(ClientKind::Standalone);
        evidence.push("GGPK 独立端布局，未发现国服或第三方平台特征".into());
    }
    let has_china = channels.contains(&ClientKind::WeGame);
    let has_intl = channels.iter().any(|c| {
        matches!(
            c,
            ClientKind::Steam | ClientKind::Epic | ClientKind::Standalone
        )
    });
    let realm = if (has_china && has_intl) || (china.len() == 1 && !has_china) {
        evidence.push("服区证据不足或冲突，保持待确认".into());
        None
    } else if has_china {
        Some(Realm::China)
    } else if has_intl {
        Some(Realm::International)
    } else {
        // Match the upstream's Intl-Bundles2 fallback, without inventing Steam/Epic.
        evidence.push("未发现国服特征，按参考项目规则判为国际服；渠道仍需平台证据".into());
        Some(Realm::International)
    };
    let channel = channels
        .first()
        .copied()
        .filter(|first| channels.iter().all(|c| c == first))
        .unwrap_or(ClientKind::Unknown);
    if channel == ClientKind::Unknown && !channels.is_empty() {
        evidence.push("启动渠道证据冲突，保持渠道待确认".into());
    }
    (channel, realm, evidence)
}

fn indexed_game(root: &Path, storage: &StorageKind) -> Result<GameId> {
    let bytes = if *storage == StorageKind::Bundles {
        let name = if root.join("Bundles2/_.index.bin").is_file() {
            "Bundles2/_.index.bin"
        } else {
            "_.index.bin"
        };
        crate::bundle::read_bounded(&crate::checked_path(root, name)?)?
    } else {
        crate::ggpk::Ggpk::open(root)?
            .read("Bundles2/_.index.bin")?
            .context("GGPK 内缺少 Bundles2 索引")?
    };
    let index = crate::bundle::BundleIndex::parse(crate::bundle::decode(&bytes)?)?;
    let mut poe1 = false;
    let mut poe2 = false;
    // CN distributions may omit the default-language table. Language is not realm evidence.
    for locale in [
        "",
        "simplified chinese/",
        "traditional chinese/",
        "japanese/",
        "korean/",
        "russian/",
        "french/",
        "german/",
        "spanish/",
        "portuguese/",
        "thai/",
    ] {
        poe1 |= index
            .lookup(&format!("data/{locale}baseitemtypes.datc64"))?
            .is_some();
        poe2 |= index
            .lookup(&format!("data/balance/{locale}baseitemtypes.datc64"))?
            .is_some();
    }
    ensure!(poe1 != poe2, "资源索引无法唯一识别 PoE1 / PoE2");
    Ok(if poe1 { GameId::Poe1 } else { GameId::Poe2 })
}

pub fn identify(path: &Path) -> Result<ClientIdentity> {
    identify_with_hints(path, &[])
}

pub fn identify_with_hints(path: &Path, hints: &[ClientHint]) -> Result<ClientIdentity> {
    let root = normalize_root(path)?;
    ensure!(
        has_resources(&root),
        "未找到游戏资源。请选择包含 Content.ggpk 或 Bundles2/_.index.bin 的客户端目录，也可选择游戏程序或资源文件。"
    );
    let storage =
        if root.join("Bundles2/_.index.bin").is_file() || root.join("_.index.bin").is_file() {
            StorageKind::Bundles
        } else {
            StorageKind::Ggpk
        };
    let (client_kind, client_realm, mut evidence) = distribution(&root, &storage, hints);
    let indexed = indexed_game(&root, &storage);
    let resource_game_verified = indexed.is_ok();
    let mut versions = Vec::new();
    for name in [
        "PathOfExile_x64Steam.exe",
        "PathOfExileSteam.exe",
        "PathOfExile_x64.exe",
        "PathOfExile.exe",
        "Client.exe",
        "PathOfExile2.exe",
        "PathOfExile2_x64.exe",
    ] {
        if let Some(major) = version::major(&root.join(name))
            && let Some(game) = game_from_major(major)
        {
            versions.push(game);
            evidence.push(format!("{name} 文件主版本 {major}：{}", game.label()));
        }
    }
    let game = match indexed {
        Ok(game) => {
            evidence.insert(
                0,
                format!(
                    "资源索引识别为 {}（优先于程序版本、安装记录与目录名称）",
                    game.label()
                ),
            );
            if versions.iter().any(|g| *g != game)
                || hints.iter().any(|h| h.game.is_some_and(|g| g != game))
            {
                evidence.push("程序版本或安装记录与资源版本不一致，以资源索引为准".into());
            }
            game
        }
        Err(error) => {
            let mut candidates = versions;
            for hint in hints {
                if let Some(game) = hint.game {
                    candidates.push(game);
                    evidence.push(format!("{}：{}", hint.source, game.label()));
                }
            }
            if root.join("poe2_helper_sdk.dll").is_file() {
                candidates.push(GameId::Poe2);
                evidence.push("poe2_helper_sdk.dll：PoE2".into());
            }
            if candidates.is_empty()
                && let Some(game) = root
                    .file_name()
                    .and_then(|n| game_from_name(&n.to_string_lossy()))
            {
                candidates.push(game);
                evidence.push(format!("客户端目录名称：{}", game.label()));
            }
            let game = candidates
                .first()
                .copied()
                .context("资源及安装信息无法识别 PoE1 / PoE2")?;
            ensure!(
                candidates.iter().all(|g| *g == game),
                "游戏版本证据冲突，无法识别 PoE1 / PoE2"
            );
            evidence.push(format!(
                "仅识别客户端身份，资源校验未通过，不授予写入能力：{error}"
            ));
            game
        }
    };
    Ok(ClientIdentity {
        root,
        game,
        storage,
        client_kind,
        client_realm,
        client_evidence: evidence,
        resource_game_verified,
    })
}
