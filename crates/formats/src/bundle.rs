//! POE bundle framing, bounded decompression and existing-path index updates.
//! The raw encoder is independently validated by oozextract. Game acceptance
//! remains a separate capability gate; successful round-trip is not that gate.
use crate::BundleHeader;
use anyhow::{Context, Result, ensure};
use std::collections::BTreeMap;

const CHUNK: usize = 0x40000;
const MAX: usize = 512 * 1024 * 1024;

pub fn decode(bytes: &[u8]) -> Result<Vec<u8>> {
    let header = BundleHeader::parse(bytes, bytes.len() as u64)?;
    let data_start = 12 + header.header_size as usize;
    ensure!(data_start <= bytes.len(), "Bundle 分块表截断");
    let mut output = vec![0u8; header.uncompressed_size as usize];
    let mut compressed_pos = data_start;
    for n in 0..header.chunks as usize {
        let pos = 60 + n * 4;
        let size = u32::from_le_bytes(bytes[pos..pos + 4].try_into()?) as usize;
        let end = compressed_pos
            .checked_add(size)
            .context("Bundle 块大小溢出")?;
        ensure!(end <= bytes.len(), "Bundle 块数据截断");
        let start = n * header.chunk_size as usize;
        let out_end = (start + header.chunk_size as usize).min(output.len());
        oozextract::Extractor::new()
            .read_from_slice(&bytes[compressed_pos..end], &mut output[start..out_end])
            .map_err(|e| anyhow::anyhow!("Oodle 块解码失败: {e}"))?;
        compressed_pos = end;
    }
    ensure!(
        compressed_pos - data_start == header.compressed_size as usize,
        "Bundle 载荷总长度不一致"
    );
    Ok(output)
}

pub fn encode_raw(bytes: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX,
        "Bundle 输入大小超出范围"
    );
    let chunks = bytes.len().div_ceil(CHUNK);
    let compressed = bytes.len() + chunks * 2;
    let mut out = Vec::with_capacity(60 + 4 * chunks + compressed);
    for n in [
        bytes.len() as u32,
        compressed as u32,
        (48 + 4 * chunks) as u32,
        8,
        1,
    ] {
        out.extend(n.to_le_bytes());
    }
    out.extend((bytes.len() as u64).to_le_bytes());
    out.extend((compressed as u64).to_le_bytes());
    out.extend((chunks as u32).to_le_bytes());
    out.extend((CHUNK as u32).to_le_bytes());
    out.extend([0u8; 16]);
    for block in bytes.chunks(CHUNK) {
        out.extend(((block.len() + 2) as u32).to_le_bytes());
    }
    for block in bytes.chunks(CHUNK) {
        out.extend([0x4c, 0x06]);
        out.extend(block);
    }
    Ok(out)
}

pub fn path_hash(path: &str) -> u64 {
    let bytes = path.to_lowercase().into_bytes();
    let m = 0xc6a4_a793_5bd1_e995u64;
    let mut h = 0x1337_b33fu64 ^ (bytes.len() as u64).wrapping_mul(m);
    let (chunks, tail) = bytes.as_chunks::<8>();
    for c in chunks {
        let mut k = u64::from_le_bytes(*c);
        k = k.wrapping_mul(m);
        k ^= k >> 47;
        k = k.wrapping_mul(m);
        h ^= k;
        h = h.wrapping_mul(m);
    }
    if !tail.is_empty() {
        for (i, b) in tail.iter().enumerate() {
            h ^= (*b as u64) << (8 * i);
        }
        h = h.wrapping_mul(m);
    }
    h ^= h >> 47;
    h = h.wrapping_mul(m);
    h ^= h >> 47;
    h
}

#[derive(Debug, Clone)]
pub struct BundleEntry {
    pub name: String,
    pub uncompressed_size: u32,
}
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub hash: u64,
    pub bundle: u32,
    pub offset: u32,
    pub size: u32,
}
pub struct BundleIndex {
    pub bundles: Vec<BundleEntry>,
    pub file_count: u32,
    raw: Vec<u8>,
    descriptor_end: usize,
    files_start: usize,
    files_end: usize,
}

/// Read one existing virtual resource from a loose Bundles2 installation.
/// Extraction never modifies the source index or bundles.
pub fn extract_resource(root: &std::path::Path, virtual_path: &str) -> Result<Vec<u8>> {
    crate::safe_relative(virtual_path)?;
    let container = if !crate::checked_path(root, "Bundles2/_.index.bin")?.is_file()
        && !crate::checked_path(root, "_.index.bin")?.is_file()
    {
        Some(crate::ggpk::Ggpk::open(root)?)
    } else {
        None
    };
    let prefix =
        if container.is_some() || crate::checked_path(root, "Bundles2/_.index.bin")?.is_file() {
            "Bundles2/"
        } else {
            ""
        };
    let read = |path: &str| -> Result<Vec<u8>> {
        if let Some(c) = &container {
            c.read(path)?.context("GGPK 中缺少资源")
        } else {
            read_bounded(&crate::checked_path(root, path)?)
        }
    };
    let index = BundleIndex::parse(decode(&read(&format!("{prefix}_.index.bin"))?)?)?;
    let entry = index.lookup(virtual_path)?.context("索引中未找到该资源")?;
    let bundle = &index.bundles[entry.bundle as usize];
    let bytes = decode(&read(&format!("{prefix}{}.bundle.bin", bundle.name))?)?;
    ensure!(
        bytes.len() == bundle.uncompressed_size as usize,
        "Bundle 解压长度与索引不一致"
    );
    Ok(bytes
        .get(entry.offset as usize..entry.offset as usize + entry.size as usize)
        .context("资源范围越界")?
        .to_vec())
}

pub(crate) fn read_bounded(path: &std::path::Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let f = std::fs::File::open(path)?;
    ensure!(f.metadata()?.len() <= MAX as u64, "资源文件超过大小上限");
    let mut bytes = Vec::new();
    f.take(MAX as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX, "资源文件超过大小上限");
    Ok(bytes)
}

fn u32_at(raw: &[u8], pos: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        raw.get(pos..pos + 4).context("索引截断")?.try_into()?,
    ))
}

impl BundleIndex {
    pub fn parse(raw: Vec<u8>) -> Result<Self> {
        ensure!(raw.len() <= MAX, "索引超出大小上限");
        let count = u32_at(&raw, 0)?;
        ensure!(count <= 1_000_000, "Bundle 数量超出上限");
        let mut bundles = Vec::new();
        let mut pos = 4;
        for _ in 0..count {
            let len = u32_at(&raw, pos)? as usize;
            pos += 4;
            ensure!(len > 0 && len < 4096, "Bundle 名称长度无效");
            let name = std::str::from_utf8(raw.get(pos..pos + len).context("Bundle 名称截断")?)?
                .to_owned();
            crate::safe_relative(&name)?;
            pos += len;
            let size = u32_at(&raw, pos)?;
            pos += 4;
            bundles.push(BundleEntry {
                name,
                uncompressed_size: size,
            });
        }
        let descriptor_end = pos;
        let file_count = u32_at(&raw, pos)?;
        pos += 4;
        ensure!(file_count <= 8_000_000, "资源数量超出上限");
        let files_start = pos;
        let files_end = pos
            .checked_add(file_count as usize * 20)
            .context("索引大小溢出")?;
        ensure!(files_end + 4 <= raw.len(), "资源记录表截断");
        let directory_count = u32_at(&raw, files_end)? as usize;
        ensure!(
            directory_count <= 1_000_000 && files_end + 4 + directory_count * 20 <= raw.len(),
            "目录记录表截断"
        );
        Ok(Self {
            bundles,
            file_count,
            raw,
            descriptor_end,
            files_start,
            files_end,
        })
    }
    fn file_entry(record: &[u8]) -> FileEntry {
        FileEntry {
            hash: u64::from_le_bytes(record[..8].try_into().unwrap()),
            bundle: u32::from_le_bytes(record[8..12].try_into().unwrap()),
            offset: u32::from_le_bytes(record[12..16].try_into().unwrap()),
            size: u32::from_le_bytes(record[16..20].try_into().unwrap()),
        }
    }
    pub fn lookup(&self, path: &str) -> Result<Option<FileEntry>> {
        let hash = path_hash(path);
        let mut found = None;
        for raw in self.raw[self.files_start..self.files_end]
            .as_chunks::<20>()
            .0
        {
            let entry = Self::file_entry(raw);
            if entry.hash != hash {
                continue;
            }
            ensure!(found.is_none(), "资源哈希重复，拒绝有歧义的映射");
            let bundle = self
                .bundles
                .get(entry.bundle as usize)
                .context("无效 Bundle 引用")?;
            ensure!(
                entry.offset as u64 + entry.size as u64 <= bundle.uncompressed_size as u64,
                "资源范围超出 Bundle"
            );
            found = Some(entry);
        }
        Ok(found)
    }
    pub fn replace_existing(
        &self,
        name: &str,
        replacements: &BTreeMap<String, Vec<u8>>,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        crate::safe_relative(name)?;
        ensure!(
            !self.bundles.iter().any(|b| b.name == name),
            "Bundle 名称已存在"
        );
        ensure!(!replacements.is_empty(), "没有资源变化");
        let mut payload = Vec::new();
        let mut updates = BTreeMap::new();
        for (path, bytes) in replacements {
            let entry = self
                .lookup(path)?
                .with_context(|| format!("索引中不存在资源 {path}；新增路径需要独立目录协议"))?;
            ensure!(
                payload.len() + bytes.len() <= MAX,
                "补丁 Bundle 超出大小上限"
            );
            updates.insert(entry.hash, (payload.len() as u32, bytes.len() as u32));
            payload.extend(bytes);
        }
        let mut raw = Vec::with_capacity(self.raw.len() + name.len() + 8);
        raw.extend(((self.bundles.len() + 1) as u32).to_le_bytes());
        raw.extend(&self.raw[4..self.descriptor_end]);
        raw.extend((name.len() as u32).to_le_bytes());
        raw.extend(name.as_bytes());
        raw.extend((payload.len() as u32).to_le_bytes());
        raw.extend(self.file_count.to_le_bytes());
        for r in self.raw[self.files_start..self.files_end]
            .as_chunks::<20>()
            .0
        {
            let e = Self::file_entry(r);
            if let Some((offset, size)) = updates.get(&e.hash) {
                raw.extend(e.hash.to_le_bytes());
                raw.extend((self.bundles.len() as u32).to_le_bytes());
                raw.extend(offset.to_le_bytes());
                raw.extend(size.to_le_bytes());
            } else {
                raw.extend(r);
            }
        }
        raw.extend(&self.raw[self.files_end..]);
        Ok((raw, payload))
    }
}
