//! Bounded read-only discovery. Launcher hints never grant write capability.
use poe2_core::{ClientKind, GameId, Installation};
use poe2_formats::client::{ClientHint, ClientIdentity, game_from_name};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
#[cfg(windows)]
mod windows;
const MAX_ENTRIES: usize = 1024;
#[derive(Debug, Clone, Serialize)]
pub struct ClientCandidate {
    #[serde(flatten)]
    pub identity: ClientIdentity,
    pub source: String,
}
#[derive(Debug, Clone, Default, Serialize)]
pub struct DiscoveryReport {
    pub scanned: bool,
    pub clients: Vec<ClientCandidate>,
    pub warnings: Vec<String>,
}
#[derive(Default)]
struct Sources {
    paths: Vec<(PathBuf, ClientHint)>,
    steam: BTreeSet<PathBuf>,
    wegame: BTreeSet<PathBuf>,
    epic: BTreeSet<PathBuf>,
}
impl Sources {
    fn path(&mut self, path: impl Into<PathBuf>, source: &'static str) {
        self.hint(path, source, None, ClientKind::Unknown);
    }
    fn hint(
        &mut self,
        path: impl Into<PathBuf>,
        source: &str,
        game: Option<GameId>,
        channel: ClientKind,
    ) {
        self.paths.push((
            path.into(),
            ClientHint {
                game,
                channel,
                source: source.into(),
            },
        ));
    }
}
fn game_name(value: &str) -> bool {
    let name = value.to_lowercase();
    name.contains("path of exile") || name.contains("流放之路") || name == "poe1" || name == "poe2"
}
fn read_text(path: &Path) -> Option<String> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > 1024 * 1024 {
        return None;
    }
    let mut text = String::new();
    file.take(1024 * 1024 + 1).read_to_string(&mut text).ok()?;
    (text.len() <= 1024 * 1024).then_some(text)
}
// Keep braces and comments separate; accept both old and modern KeyValues.
fn vdf_pairs(text: &str) -> Vec<(String, String)> {
    let mut tokens: Vec<Option<String>> = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '/' if chars.peek() == Some(&'/') => {
                for ch in chars.by_ref() {
                    if ch == '\n' {
                        break;
                    }
                }
            }
            '{' | '}' => tokens.push(None),
            '"' => {
                let mut value = String::new();
                let mut closed = false;
                while let Some(ch) = chars.next() {
                    if ch == '"' {
                        closed = true;
                        break;
                    }
                    if ch == '\\' && chars.peek().is_some_and(|c| *c == '\\' || *c == '"') {
                        value.push(chars.next().unwrap());
                    } else {
                        value.push(ch);
                    }
                }
                if !closed {
                    return Vec::new();
                }
                tokens.push(Some(value));
            }
            _ => {}
        }
    }
    let mut pairs = Vec::new();
    let mut index = 0;
    while index + 1 < tokens.len() {
        if let (Some(key), Some(value)) = (&tokens[index], &tokens[index + 1]) {
            pairs.push((key.to_ascii_lowercase(), value.clone()));
            index += 2;
        } else {
            index += 1;
        }
    }
    pairs
}
/// Normalize a registry path or DisplayIcon; never execute its arguments.
fn configured_path(value: &str) -> Option<PathBuf> {
    let value = value.trim();
    if value.is_empty() || value.len() > 4096 {
        return None;
    }
    let value = if let Some(quoted) = value.strip_prefix('"') {
        quoted.split_once('"')?.0
    } else if let Some((path, suffix)) = value.rsplit_once(',') {
        if suffix.trim().parse::<i32>().is_ok() {
            path
        } else {
            value
        }
    } else {
        value
    };
    let mut expanded = String::new();
    let mut tail = value;
    while let Some((before, rest)) = tail.split_once('%') {
        expanded.push_str(before);
        let (key, after) = rest.split_once('%')?;
        expanded.push_str(&std::env::var(key).ok()?);
        tail = after;
    }
    expanded.push_str(tail);
    if !Path::new(&expanded).exists()
        && let Some(end) = expanded.to_ascii_lowercase().find(".exe")
    {
        expanded.truncate(end + 4);
    }
    let path = PathBuf::from(expanded);
    path.is_absolute().then_some(path)
}
fn steam_paths(root: &Path, sources: &mut Sources) {
    let mut libraries = BTreeSet::from([root.to_path_buf()]);
    if let Some(text) = read_text(&root.join("steamapps/libraryfolders.vdf")) {
        for (key, value) in vdf_pairs(&text) {
            if (key == "path" || key.parse::<u32>().is_ok()) && Path::new(&value).is_absolute() {
                libraries.insert(PathBuf::from(value));
            }
        }
    }
    for library in libraries.into_iter().take(64) {
        let apps = library.join("steamapps");
        for name in ["Path of Exile", "Path of Exile 2"] {
            sources.hint(
                apps.join("common").join(name),
                "Steam 游戏库",
                game_from_name(name),
                ClientKind::Steam,
            );
        }
        let Ok(entries) = std::fs::read_dir(&apps) else {
            continue;
        };
        for entry in entries.take(MAX_ENTRIES).flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("appmanifest_") || !name.ends_with(".acf") {
                continue;
            }
            let Some(text) = read_text(&entry.path()) else {
                continue;
            };
            let pairs = vdf_pairs(&text);
            if !pairs.iter().any(|(k, v)| k == "name" && game_name(v)) {
                continue;
            }
            if let Some((_, directory)) = pairs.iter().find(|(k, _)| k == "installdir")
                && let Ok(relative) = poe2_formats::safe_relative(directory)
            {
                let game = pairs
                    .iter()
                    .find(|(k, _)| k == "name")
                    .and_then(|(_, n)| game_from_name(n));
                sources.hint(
                    apps.join("common").join(relative),
                    "Steam 安装清单",
                    game,
                    ClientKind::Steam,
                );
            }
        }
    }
}
fn wegame_paths(base: &Path, sources: &mut Sources) {
    let base = if base.is_file() {
        base.parent().unwrap_or(base)
    } else {
        base
    };
    let mut roots = BTreeSet::from([
        base.to_path_buf(),
        base.join("rail_apps"),
        base.join("WeGameApps/rail_apps"),
    ]);
    if let Some(parent) = base.parent() {
        roots.insert(parent.join("WeGameApps/rail_apps"));
    }
    for root in roots {
        sources.hint(&root, "WeGame 游戏库", None, ClientKind::WeGame);
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.take(MAX_ENTRIES).flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                sources.hint(
                    entry.path(),
                    "WeGame 游戏库",
                    game_from_name(&entry.file_name().to_string_lossy()),
                    ClientKind::WeGame,
                );
            }
        }
    }
}
fn epic_paths(directory: &Path, sources: &mut Sources) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.take(MAX_ENTRIES).flatten() {
        if entry.path().extension().is_none_or(|e| e != "item") {
            continue;
        }
        let Some(text) = read_text(&entry.path()) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if value["DisplayName"].as_str().is_some_and(game_name)
            && let Some(path) = value["InstallLocation"].as_str().and_then(configured_path)
        {
            sources.hint(
                path,
                "Epic 安装清单",
                value["DisplayName"].as_str().and_then(game_from_name),
                ClientKind::Epic,
            );
        }
    }
}
fn system_sources(saved: Vec<PathBuf>) -> Sources {
    let mut sources = Sources::default();
    for path in saved {
        sources.path(path, "已保存的客户端");
    }
    for name in ["POE1_GAME_DIR", "POE1_DIR", "POE2_GAME_DIR", "POE2_DIR"] {
        if let Some(path) = std::env::var_os(name) {
            sources.path(PathBuf::from(path), "游戏目录环境变量");
        }
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        sources.path(parent, "工具所在目录");
    }
    for name in ["WEGAME_GAME_ROOT", "WEGAME_APPS_ROOT", "TENCENT_GAME_ROOT"] {
        if let Some(path) = std::env::var_os(name) {
            sources.wegame.insert(PathBuf::from(path));
        }
    }
    for variable in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Some(base) = std::env::var_os(variable) {
            let base = PathBuf::from(base);
            for name in ["Path of Exile", "Path of Exile 2"] {
                for prefix in ["", "Grinding Gear Games", "Programs"] {
                    sources.path(base.join(prefix).join(name), "常见安装目录");
                }
            }
            sources.steam.insert(base.join("Steam"));
            sources.wegame.insert(base.join("WeGameApps/rail_apps"));
            sources.wegame.insert(base.join("Tencent/WeGame"));
        }
    }
    if let Some(root) = std::env::var_os("ProgramData") {
        sources
            .epic
            .insert(PathBuf::from(root).join("Epic/EpicGamesLauncher/Data/Manifests"));
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        sources
            .steam
            .insert(PathBuf::from(home).join("Library/Application Support/Steam"));
    }
    #[cfg(target_os = "linux")]
    if let Some(home) = std::env::var_os("HOME") {
        sources
            .steam
            .insert(PathBuf::from(&home).join(".local/share/Steam"));
        sources
            .steam
            .insert(PathBuf::from(home).join(".steam/steam"));
    }
    #[cfg(windows)]
    windows::collect(&mut sources);
    sources
}
fn all_sources(saved: Vec<PathBuf>) -> Sources {
    let mut sources = system_sources(saved);
    for path in sources.steam.clone() {
        steam_paths(&path, &mut sources);
    }
    for path in sources.wegame.clone() {
        wegame_paths(&path, &mut sources);
    }
    for path in sources.epic.clone() {
        epic_paths(&path, &mut sources);
    }
    sources
}
pub fn discover(saved: Vec<PathBuf>) -> DiscoveryReport {
    probe(all_sources(saved).paths)
}
/// Re-read matching launcher records for manual selections as well as discovered rows.
pub fn inspect(path: &Path) -> anyhow::Result<Installation> {
    let root = poe2_formats::client::normalize_root(path)?;
    let hints = all_sources(Vec::new())
        .paths
        .into_iter()
        .filter_map(|(path, hint)| {
            (poe2_formats::client::normalize_root(&path).ok().as_ref() == Some(&root))
                .then_some(hint)
        })
        .collect::<Vec<_>>();
    inspect_identity(poe2_formats::client::identify_with_hints(&root, &hints)?)
}
fn inspect_identity(identity: ClientIdentity) -> anyhow::Result<Installation> {
    let mut installation = poe2_formats::inspect_for_game(&identity.root, identity.game)?;
    installation.client_kind = identity.client_kind;
    installation.client_realm = identity.client_realm;
    installation.client_evidence = identity.client_evidence;
    installation.can_write &= identity.resource_game_verified;
    Ok(installation)
}
fn probe(paths: Vec<(PathBuf, ClientHint)>) -> DiscoveryReport {
    let mut report = DiscoveryReport {
        scanned: true,
        ..Default::default()
    };
    // Aggregate every launcher record for a root before classifying it. Otherwise
    // an earlier saved-path hint would hide Steam/Epic/WeGame evidence or conflicts.
    let mut grouped: BTreeMap<String, (PathBuf, Vec<ClientHint>)> = BTreeMap::new();
    for (path, source) in paths {
        let Ok(root) = poe2_formats::client::normalize_root(&path) else {
            continue;
        };
        if ![
            "Bundles2/_.index.bin",
            "_.index.bin",
            "Content.ggpk",
            "content.ggpk",
        ]
        .iter()
        .any(|name| root.join(name).is_file())
        {
            continue;
        }
        let key = root.to_string_lossy().into_owned();
        let key = if cfg!(windows) {
            key.to_lowercase()
        } else {
            key
        };
        grouped
            .entry(key)
            .or_insert_with(|| (root, Vec::new()))
            .1
            .push(source);
    }
    for (probed, (_, (root, hints))) in grouped.into_iter().enumerate() {
        if probed == 128 {
            report
                .warnings
                .push("候选客户端达到本次检查上限，其余目录可手动选择。".into());
            break;
        }
        match poe2_formats::client::identify_with_hints(&root, &hints) {
            Ok(identity) => report.clients.push(ClientCandidate {
                identity,
                source: hints
                    .iter()
                    .map(|h| h.source.as_str())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join("、"),
            }),
            Err(error) if report.warnings.len() < 16 => {
                report.warnings.push(format!("{}：{error}", root.display()))
            }
            Err(_) => {}
        }
    }
    report
}
pub fn installations() -> Vec<Installation> {
    discover(Vec::new())
        .clients
        .into_iter()
        .filter_map(|candidate| inspect_identity(candidate.identity).ok())
        .collect()
}
