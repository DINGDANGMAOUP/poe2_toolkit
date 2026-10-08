//! GGPK 3/4 reader and copy-on-write reconstruction. Original records remain
//! unchanged; new FILE/PDIR records are appended and a new root is published in
//! a separately staged container. No in-place pointer writes are performed.
use anyhow::{Context, Result, bail, ensure};
use poe2_core::{StreamPiece, StreamRecipe, digest};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Baseline {
    pub length: u64,
    pub root: u64,
    pub hash: String,
}
#[derive(Clone)]
struct Child {
    hash: u32,
    offset: u64,
}
struct Record {
    name: String,
    hash: Vec<u8>,
    children: Option<Vec<Child>>,
    data_offset: u64,
    data_len: u64,
}
pub(crate) struct Ggpk {
    pub path: PathBuf,
    pub relative: String,
    pub length: u64,
    pub root: u64,
    version: u32,
}
impl Ggpk {
    pub fn open(root: &Path) -> Result<Self> {
        let relative = if root.join("Content.ggpk").is_file() {
            "Content.ggpk"
        } else {
            "content.ggpk"
        }
        .to_owned();
        let path = crate::checked_path(root, &relative)?;
        let mut file = File::open(&path)?;
        let length = file.metadata()?.len();
        ensure!(
            (28..=512 * 1024 * 1024 * 1024).contains(&length),
            "GGPK 容器长度超限"
        );
        let mut header = [0; 28];
        file.read_exact(&mut header)?;
        ensure!(
            &header[4..8] == b"GGPK" && u32::from_le_bytes(header[..4].try_into()?) == 28,
            "GGPK 头部无效"
        );
        let version = u32::from_le_bytes(header[8..12].try_into()?);
        ensure!(
            matches!(version, 3 | 4),
            "只支持包含 Bundles2 的 GGPK v3/v4"
        );
        let root = u64::from_le_bytes(header[12..20].try_into()?);
        let container = Self {
            path,
            relative,
            length,
            root,
            version,
        };
        ensure!(
            container.record(root)?.children.is_some(),
            "GGPK 根记录不是目录"
        );
        Ok(container)
    }
    fn bytes(&self, offset: u64, len: usize) -> Result<Vec<u8>> {
        ensure!(
            len <= 512 * 1024 * 1024
                && offset
                    .checked_add(len as u64)
                    .is_some_and(|end| end <= self.length),
            "GGPK 读取范围越界"
        );
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; len];
        file.read_exact(&mut bytes)?;
        Ok(bytes)
    }
    fn record(&self, offset: u64) -> Result<Record> {
        ensure!(offset >= 28, "GGPK 节点指向头部");
        let header = self.bytes(offset, 12)?;
        let length = u32::from_le_bytes(header[..4].try_into()?) as u64;
        ensure!(
            length >= 12
                && offset
                    .checked_add(length)
                    .is_some_and(|end| end <= self.length),
            "GGPK 节点截断"
        );
        let directory = &header[4..8] == b"PDIR";
        ensure!(directory || &header[4..8] == b"FILE", "GGPK 子节点类型未知");
        let chars = u32::from_le_bytes(header[8..12].try_into()?) as usize;
        ensure!((1..=4096).contains(&chars), "GGPK 名称长度无效");
        let fixed = if directory { 48 } else { 44 };
        let unit = if self.version == 4 { 4 } else { 2 };
        let name_end = fixed + chars * unit;
        ensure!(name_end as u64 <= length, "GGPK 名称截断");
        let raw = self.bytes(offset, name_end)?;
        let name_bytes = &raw[fixed..name_end];
        ensure!(
            name_bytes[name_bytes.len() - unit..]
                .iter()
                .all(|b| *b == 0),
            "GGPK 名称未终止"
        );
        let name = if unit == 2 {
            String::from_utf16(
                &name_bytes[..name_bytes.len() - 2]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|x| u16::from_le_bytes(*x))
                    .collect::<Vec<_>>(),
            )?
        } else {
            name_bytes[..name_bytes.len() - 4]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|x| char::from_u32(u32::from_le_bytes(*x)).context("GGPK UTF-32 无效"))
                .collect::<Result<String>>()?
        };
        ensure!(!name.contains(['\0', '/', '\\']), "GGPK 节点名称不合法");
        let hash = raw[if directory { 16 } else { 12 }..fixed].to_vec();
        let children = if directory {
            let count = u32::from_le_bytes(raw[12..16].try_into()?) as usize;
            ensure!(
                count <= 1_000_000 && name_end as u64 + count as u64 * 12 == length,
                "GGPK 目录长度不一致"
            );
            let entries = self.bytes(offset + name_end as u64, count * 12)?;
            let mut children = Vec::new();
            let mut previous = None;
            for entry in entries.as_chunks::<12>().0 {
                let hash = u32::from_le_bytes(entry[..4].try_into()?);
                ensure!(
                    previous.is_none_or(|old| old < hash),
                    "GGPK 子节点哈希重复或无序"
                );
                previous = Some(hash);
                let child = u64::from_le_bytes(entry[4..].try_into()?);
                ensure!(
                    child != offset && child >= 28 && child < self.length,
                    "GGPK 子节点引用无效"
                );
                children.push(Child {
                    hash,
                    offset: child,
                });
            }
            Some(children)
        } else {
            None
        };
        Ok(Record {
            name,
            hash,
            children,
            data_offset: offset + name_end as u64,
            data_len: length - name_end as u64,
        })
    }
    fn lookup(&self, path: &str) -> Result<Option<Record>> {
        crate::safe_relative(path)?;
        let mut current = self.record(self.root)?;
        let mut seen = BTreeSet::from([self.root]);
        ensure!(path.split('/').count() <= 64, "GGPK 路径过深");
        for component in path.split('/') {
            self.validate_directory(&current)?;
            let children = current
                .children
                .as_ref()
                .context("GGPK 路径经过非目录节点")?;
            let hash = name_hash(component);
            let Ok(n) = children.binary_search_by_key(&hash, |c| c.hash) else {
                return Ok(None);
            };
            let offset = children[n].offset;
            ensure!(seen.insert(offset), "GGPK 目录循环引用");
            current = self.record(offset)?;
            ensure!(
                current.name.to_lowercase() == component.to_lowercase(),
                "GGPK 名称哈希冲突"
            );
        }
        Ok(Some(current))
    }
    pub fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let Some(record) = self.lookup(path)? else {
            return Ok(None);
        };
        ensure!(record.children.is_none(), "GGPK 目标不是文件");
        let bytes = self.bytes(record.data_offset, usize::try_from(record.data_len)?)?;
        ensure!(
            digest(&bytes) == hex::encode(&record.hash),
            "GGPK 文件 SHA-256 校验失败：{path}"
        );
        Ok(Some(bytes))
    }
    pub fn baseline(&self) -> Result<Baseline> {
        Ok(Baseline {
            length: self.length,
            root: self.root,
            hash: crate::hash_file(&self.path)?,
        })
    }
    pub fn restore(&self, baseline: &Baseline) -> Result<(StreamRecipe, String)> {
        ensure!(
            baseline.length >= 28
                && baseline.length <= self.length
                && baseline.root < baseline.length,
            "GGPK 基线长度或根记录无效"
        );
        let recipe = StreamRecipe {
            source_hash: crate::hash_file(&self.path)?,
            source_len: self.length,
            pieces: vec![
                StreamPiece::Copy {
                    offset: 0,
                    length: 12,
                },
                StreamPiece::Bytes {
                    data: baseline.root.to_le_bytes().to_vec(),
                },
                StreamPiece::Copy {
                    offset: 20,
                    length: baseline.length - 20,
                },
            ],
        };
        ensure!(
            crate::stream::reconstruct(&self.path, &recipe, &mut std::io::sink())? == baseline.hash,
            "GGPK 原始区域已被外部修改，不能精确清除"
        );
        Ok((recipe, baseline.hash.clone()))
    }
    pub fn rewrite(
        &self,
        replacements: &BTreeMap<String, Vec<u8>>,
    ) -> Result<(StreamRecipe, String)> {
        let mut distinct = BTreeSet::new();
        for path in replacements.keys() {
            crate::safe_relative(path)?;
            ensure!(path.split('/').count() <= 64, "GGPK 输出路径过深");
            ensure!(distinct.insert(path.to_lowercase()), "GGPK 输出路径重复");
            if let Some((parent, _)) = path.rsplit_once('/') {
                ensure!(
                    self.lookup(parent)?.is_some_and(|r| r.children.is_some()),
                    "GGPK 输出父目录不存在"
                );
            }
        }
        let mut append = Vec::new();
        let (root, _) = self.rewrite_directory(
            self.root,
            "",
            replacements,
            &mut append,
            &mut BTreeSet::new(),
        )?;
        let recipe = StreamRecipe {
            source_hash: crate::hash_file(&self.path)?,
            source_len: self.length,
            pieces: vec![
                StreamPiece::Copy {
                    offset: 0,
                    length: 12,
                },
                StreamPiece::Bytes {
                    data: root.to_le_bytes().to_vec(),
                },
                StreamPiece::Copy {
                    offset: 20,
                    length: self.length - 20,
                },
                StreamPiece::Bytes { data: append },
            ],
        };
        let hash = crate::stream::reconstruct(&self.path, &recipe, &mut std::io::sink())?;
        Ok((recipe, hash))
    }
    fn rewrite_directory(
        &self,
        offset: u64,
        prefix: &str,
        replacements: &BTreeMap<String, Vec<u8>>,
        append: &mut Vec<u8>,
        seen: &mut BTreeSet<u64>,
    ) -> Result<(u64, Vec<u8>)> {
        ensure!(
            seen.len() < 64 && seen.insert(offset),
            "GGPK 目录循环或过深"
        );
        let record = self.record(offset)?;
        self.validate_directory(&record)?;
        let children = record.children.as_ref().context("GGPK 路径不是目录")?;
        let mut entries = BTreeMap::<u32, (u64, Vec<u8>)>::new();
        let mut consumed = BTreeSet::new();
        for child in children {
            let item = self.record(child.offset)?;
            ensure!(
                name_hash(&item.name) == child.hash,
                "GGPK 子节点名称哈希不符"
            );
            let path = format!("{prefix}{}", item.name);
            let exact = replacements.keys().find(|p| p.eq_ignore_ascii_case(&path));
            let nested = format!("{}/", path.to_lowercase());
            let (at, hash) = if let Some(key) = exact {
                ensure!(item.children.is_none(), "不能用文件替换目录");
                consumed.insert(key.clone());
                self.append_file(&item.name, &replacements[key], append)?
            } else if replacements
                .keys()
                .any(|p| p.to_lowercase().starts_with(&nested))
            {
                self.rewrite_directory(
                    child.offset,
                    &format!("{path}/"),
                    replacements,
                    append,
                    seen,
                )?
            } else {
                (child.offset, item.hash)
            };
            entries.insert(child.hash, (at, hash));
        }
        for (path, bytes) in replacements {
            if !path.to_lowercase().starts_with(&prefix.to_lowercase()) {
                continue;
            }
            let remaining = &path[prefix.len()..];
            if remaining.contains('/') || consumed.contains(path) {
                continue;
            }
            let hash = name_hash(remaining);
            ensure!(!entries.contains_key(&hash), "GGPK 新资源名称冲突");
            let entry = self.append_file(remaining, bytes, append)?;
            entries.insert(hash, entry);
        }
        let hashes: Vec<_> = entries
            .values()
            .flat_map(|(_, h)| h.iter().copied())
            .collect();
        let hash = hex::decode(digest(&hashes))?;
        let name = self.encode_name(&record.name);
        let len = 48 + name.len() + entries.len() * 12;
        let at = self.length + append.len() as u64;
        append.extend(u32::try_from(len)?.to_le_bytes());
        append.extend(b"PDIR");
        append.extend(
            u32::try_from(name.len() / if self.version == 4 { 4 } else { 2 })?.to_le_bytes(),
        );
        append.extend(u32::try_from(entries.len())?.to_le_bytes());
        append.extend(&hash);
        append.extend(name);
        for (name_hash, (offset, _)) in entries {
            append.extend(name_hash.to_le_bytes());
            append.extend(offset.to_le_bytes());
        }
        ensure!(append.len() <= 512 * 1024 * 1024, "GGPK 变更总量超限");
        seen.remove(&offset);
        Ok((at, hash))
    }
    fn encode_name(&self, name: &str) -> Vec<u8> {
        if self.version == 4 {
            name.chars()
                .map(|c| c as u32)
                .chain([0])
                .flat_map(u32::to_le_bytes)
                .collect()
        } else {
            name.encode_utf16()
                .chain([0])
                .flat_map(u16::to_le_bytes)
                .collect()
        }
    }
    fn append_file(
        &self,
        name: &str,
        bytes: &[u8],
        append: &mut Vec<u8>,
    ) -> Result<(u64, Vec<u8>)> {
        if append.len() + bytes.len() + 44 + name.len() * 4 + 4 > 512 * 1024 * 1024 {
            bail!("GGPK 资源超限");
        }
        let hash = hex::decode(digest(bytes))?;
        let name = self.encode_name(name);
        let len = 44 + name.len() + bytes.len();
        let at = self.length + append.len() as u64;
        append.extend(u32::try_from(len)?.to_le_bytes());
        append.extend(b"FILE");
        append.extend(
            u32::try_from(name.len() / if self.version == 4 { 4 } else { 2 })?.to_le_bytes(),
        );
        append.extend(&hash);
        append.extend(name);
        append.extend(bytes);
        Ok((at, hash))
    }
    fn validate_directory(&self, record: &Record) -> Result<()> {
        let children = record.children.as_ref().context("GGPK 节点不是目录")?;
        let mut hashes = Vec::with_capacity(children.len() * 32);
        for child in children {
            let item = self.record(child.offset)?;
            ensure!(
                name_hash(&item.name) == child.hash,
                "GGPK 子节点名称哈希不符"
            );
            hashes.extend(item.hash);
        }
        ensure!(
            digest(&hashes) == hex::encode(&record.hash),
            "GGPK 目录 SHA-256 校验失败"
        );
        Ok(())
    }
}
fn name_hash(name: &str) -> u32 {
    let bytes: Vec<_> = name
        .to_lowercase()
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut h = bytes.len() as u32;
    let multiplier = 0x5bd1e995u32;
    let (words, tail) = bytes.as_chunks::<4>();
    for word in words {
        let mut k = u32::from_le_bytes(*word).wrapping_mul(multiplier);
        k ^= k >> 24;
        k = k.wrapping_mul(multiplier);
        h = h.wrapping_mul(multiplier) ^ k;
    }
    for (n, b) in tail.iter().enumerate() {
        h ^= (*b as u32) << (8 * n);
    }
    if !tail.is_empty() {
        h = h.wrapping_mul(multiplier);
    }
    h ^= h >> 13;
    h = h.wrapping_mul(multiplier);
    h ^ (h >> 15)
}
