//! Read only both registry views; no elevation, subprocesses or registry writes.
use super::{ClientKind, MAX_ENTRIES, Sources, configured_path, game_from_name, game_name};
use winreg::{RegKey, enums::*};

pub(super) fn collect(sources: &mut Sources) {
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let hive = RegKey::predef(hive);
        for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            for game in ["Path of Exile", "Path of Exile 2"] {
                if let Ok(key) = hive.open_subkey_with_flags(
                    format!("Software\\GrindingGearGames\\{game}"),
                    KEY_READ | view,
                ) {
                    for field in ["InstallLocation", "InstallPath", "Path", "GamePath"] {
                        if let Ok(value) = key.get_value::<String, _>(field)
                            && let Some(path) = configured_path(&value)
                        {
                            sources.hint(
                                path,
                                "GGG 安装记录",
                                game_from_name(game),
                                ClientKind::Standalone,
                            );
                        }
                    }
                }
            }
            if let Ok(key) = hive.open_subkey_with_flags("Software\\Valve\\Steam", KEY_READ | view)
            {
                for field in ["SteamPath", "InstallPath"] {
                    if let Ok(value) = key.get_value::<String, _>(field)
                        && let Some(path) = configured_path(&value)
                    {
                        sources.steam.insert(path);
                    }
                }
            }
            if let Ok(key) =
                hive.open_subkey_with_flags("Software\\Tencent\\WeGame", KEY_READ | view)
            {
                for field in [
                    "InstallPath",
                    "InstallLocation",
                    "Path",
                    "RootPath",
                    "GamePath",
                    "GameRoot",
                ] {
                    if let Ok(value) = key.get_value::<String, _>(field)
                        && let Some(path) = configured_path(&value)
                    {
                        sources.wegame.insert(path);
                    }
                }
            }
            if let Ok(key) = hive.open_subkey_with_flags(
                "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
                KEY_READ | view,
            ) {
                for name in key.enum_keys().take(MAX_ENTRIES).flatten() {
                    let Ok(entry) = key.open_subkey_with_flags(name, KEY_READ | view) else {
                        continue;
                    };
                    let Ok(name) = entry.get_value::<String, _>("DisplayName") else {
                        continue;
                    };
                    let is_game = game_name(&name);
                    let is_wegame =
                        name.to_lowercase().contains("wegame") || name.contains("腾讯游戏平台");
                    if !is_game && !is_wegame {
                        continue;
                    }
                    for field in ["InstallLocation", "DisplayIcon", "InstallSource"] {
                        if let Ok(value) = entry.get_value::<String, _>(field)
                            && let Some(path) = configured_path(&value)
                        {
                            if is_game {
                                let channel = if name.to_lowercase().contains("wegame") {
                                    ClientKind::WeGame
                                } else if name.to_lowercase().contains("steam") {
                                    ClientKind::Steam
                                } else if name.to_lowercase().contains("epic") {
                                    ClientKind::Epic
                                } else {
                                    ClientKind::Unknown
                                };
                                sources.hint(
                                    path.clone(),
                                    "已安装程序记录",
                                    game_from_name(&name),
                                    channel,
                                );
                            }
                            if is_wegame {
                                sources.wegame.insert(path);
                            }
                        }
                    }
                }
            }
        }
    }
}
