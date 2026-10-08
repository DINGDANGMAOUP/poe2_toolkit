use anyhow::{Context, Result, ensure};
use poe2_core::{Installation, StorageKind, digest};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
};

pub fn safe_relative(path: &str) -> Result<PathBuf> {
    ensure!(
        !path.is_empty()
            && path.len() < 4096
            && !path.contains('\\')
            && !path.contains(':')
            && !path.contains('\0'),
        "资源路径不合法"
    );
    ensure!(
        path.split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".."),
        "资源路径包含空段或相对跳转"
    );
    let path = Path::new(path);
    ensure!(
        path.components().all(|p| matches!(p, Component::Normal(_))),
        "资源路径不能越过安装目录"
    );
    for part in path.components() {
        let text = part.as_os_str().to_string_lossy();
        ensure!(!text.ends_with(['.', ' ']), "资源路径不能以句点或空格结尾");
        let stem = text.split('.').next().unwrap_or("").to_ascii_uppercase();
        ensure!(
            !matches!(
                stem.as_str(),
                "CON"
                    | "PRN"
                    | "AUX"
                    | "NUL"
                    | "COM1"
                    | "COM2"
                    | "COM3"
                    | "COM4"
                    | "COM5"
                    | "COM6"
                    | "COM7"
                    | "COM8"
                    | "COM9"
                    | "LPT1"
                    | "LPT2"
                    | "LPT3"
                    | "LPT4"
                    | "LPT5"
                    | "LPT6"
                    | "LPT7"
                    | "LPT8"
                    | "LPT9"
            ),
            "资源路径包含设备名称"
        );
    }
    Ok(path.to_path_buf())
}

pub fn checked_path(root: &Path, relative: &str) -> Result<PathBuf> {
    let rel = safe_relative(relative)?;
    let root = std::fs::canonicalize(root)?;
    let mut path = root.clone();
    for component in rel.components() {
        path.push(component);
        if let Ok(meta) = std::fs::symlink_metadata(&path) {
            ensure!(!meta.file_type().is_symlink(), "资源路径包含符号链接");
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                ensure!(meta.file_attributes() & 0x400 == 0, "资源路径包含重解析点");
            }
            ensure!(
                std::fs::canonicalize(&path)?.starts_with(&root),
                "资源路径越界"
            );
        }
    }
    Ok(path)
}

pub fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn inspect(path: &Path) -> Result<Installation> {
    inspect_for_game(path, poe2_core::GameId::Poe2)
}

pub fn inspect_for_game(path: &Path, game: poe2_core::GameId) -> Result<Installation> {
    let root = crate::client::normalize_root(path)?;
    let identity = crate::client::identify(&root).ok();
    if let Some(identity) = &identity {
        ensure!(
            identity.game == game,
            "所选资源属于 {}，请先切换至对应游戏工作区",
            identity.game.label()
        );
    }
    let (storage, fingerprint, can_read, mut reasons) = {
        let index = if root.join("Bundles2/_.index.bin").is_file() {
            Some(root.join("Bundles2/_.index.bin"))
        } else if root.join("_.index.bin").is_file() {
            Some(root.join("_.index.bin"))
        } else {
            None
        };
        if let Some(index) = index {
            let mut file = File::open(&index)?;
            let mut header = [0u8; 60];
            file.read_exact(&mut header)?;
            let _ = BundleHeader::parse(&header, file.metadata()?.len())?;
            (
                StorageKind::Bundles,
                hash_file(&index)?,
                true,
                vec!["识别到 Bundles2；规划时逐表核验版本与物品身份".into()],
            )
        } else {
            let ggpk = ["Content.ggpk", "content.ggpk"]
                .into_iter()
                .map(|n| root.join(n))
                .find(|p| p.is_file());
            if let Some(path) = ggpk {
                let verified = (|| -> Result<String> {
                    let container = crate::ggpk::Ggpk::open(&root)?;
                    let mut index = container
                        .read("Bundles2/_.index.bin")?
                        .context("GGPK 内未找到 Bundles2 索引")?;
                    crate::bundle::BundleIndex::parse(crate::bundle::decode(&index)?)?;
                    index.extend(container.length.to_le_bytes());
                    index.extend(container.root.to_le_bytes());
                    Ok(digest(&index))
                })();
                match verified {
                    Ok(fingerprint) => (StorageKind::Ggpk, fingerprint, true,
                        vec!["已校验 GGPK v3/v4 与 Bundles2 索引；写入采用完整容器暂存替换，需预留备份空间".into()]),
                    Err(error) => (StorageKind::Ggpk, digest(&std::fs::metadata(path)?.len().to_le_bytes()), false,
                        vec![format!("GGPK 暂不可用：{error}")]),
                }
            } else {
                (
                    StorageKind::Unknown,
                    String::new(),
                    false,
                    vec!["未找到 Content.ggpk 或 Bundles2/_.index.bin".into()],
                )
            }
        }
    };
    let resource_identity_verified = identity.as_ref().is_some_and(|i| i.resource_game_verified);
    if !resource_identity_verified {
        reasons.push("客户端身份线索不能代替资源校验；未确认资源所属游戏，禁止写入".into());
    }
    let can_write = if can_read
        && resource_identity_verified
        && matches!(storage, StorageKind::Bundles | StorageKind::Ggpk)
    {
        match crate::resource::writable_game(&root, game) {
            Ok(()) => true,
            Err(error) => {
                reasons.push(format!("写入条件不满足：{error}"));
                false
            }
        }
    } else {
        false
    };
    let (client_kind, client_realm, client_evidence) = identity
        .map(|i| (i.client_kind, i.client_realm, i.client_evidence))
        .unwrap_or_default();
    Ok(Installation {
        game,
        id: if game == poe2_core::GameId::Poe2 {
            digest(root.to_string_lossy().as_bytes())
        } else {
            digest(format!("{}:{}", game.key(), root.display()).as_bytes())
        },
        root,
        storage,
        client_kind,
        client_realm,
        client_evidence,
        fingerprint,
        can_read,
        can_write,
        reasons,
    })
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BundleHeader {
    pub uncompressed_size: u64,
    pub compressed_size: u64,
    pub header_size: u32,
    pub codec: u32,
    pub chunks: u32,
    pub chunk_size: u32,
}

impl BundleHeader {
    pub fn parse(bytes: &[u8], file_size: u64) -> Result<Self> {
        ensure!(bytes.len() >= 60, "Bundle 头部截断");
        let u32_at = |p| u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap());
        let u64_at = |p| u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap());
        let header = Self {
            uncompressed_size: u64_at(20),
            compressed_size: u64_at(28),
            header_size: u32_at(8),
            codec: u32_at(12),
            chunks: u32_at(36),
            chunk_size: u32_at(40),
        };
        ensure!(
            header.uncompressed_size > 0 && header.uncompressed_size <= 512 * 1024 * 1024,
            "Bundle 解压大小超出支持范围"
        );
        ensure!(
            header.chunks > 0
                && header.chunks <= 131072
                && header.chunk_size > 0
                && header.chunk_size <= 4 * 1024 * 1024,
            "Bundle 分块参数不合法"
        );
        ensure!(
            header.header_size as u64 == 48 + 4 * header.chunks as u64,
            "Bundle 头部长度不匹配"
        );
        ensure!(
            12 + header.header_size as u64 + header.compressed_size <= file_size,
            "Bundle 数据截断"
        );
        ensure!(
            header.uncompressed_size.div_ceil(header.chunk_size as u64) == header.chunks as u64,
            "Bundle 分块数量不匹配"
        );
        Ok(header)
    }
}
