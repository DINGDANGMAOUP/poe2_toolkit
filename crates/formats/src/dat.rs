//! Validated DAT string-table access. Layout inference is read-only evidence,
//! not a capability grant to write an unknown game version.
use anyhow::{Context, Result, ensure};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, serde::Serialize)]
pub struct DatLayout {
    pub rows: u32,
    pub row_size: usize,
    pub string_base: usize,
    pub name_offset: usize,
}
#[derive(Debug, Clone, serde::Serialize)]
pub struct DatRecord {
    pub row: u32,
    pub identity: String,
    pub display: String,
    pub name_pointer: usize,
}
#[derive(Debug)]
pub struct DatTable {
    pub layout: DatLayout,
    pub records: Vec<DatRecord>,
    bytes: Vec<u8>,
}

fn pointer(bytes: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        bytes.get(at..at + 4).context("DAT 指针截断")?.try_into()?,
    ))
}
fn string(bytes: &[u8], base: usize, relative: u32) -> Result<String> {
    ensure!(relative.is_multiple_of(2), "DAT 字符串偏移未对齐");
    let start = base
        .checked_add(relative as usize)
        .context("DAT 偏移溢出")?;
    let source = bytes.get(start..).context("DAT 字符串越界")?;
    let mut units = Vec::new();
    for chunk in source.as_chunks::<2>().0.iter().take(4097) {
        let unit = u16::from_le_bytes([chunk[0], chunk[1]]);
        if unit == 0 {
            return String::from_utf16(&units).context("DAT 字符串不是有效 UTF-16");
        }
        units.push(unit);
    }
    anyhow::bail!("DAT 字符串过长或未终止")
}

impl DatTable {
    pub fn base_items(bytes: Vec<u8>) -> Result<Self> {
        ensure!(
            bytes.len() >= 40 && bytes.len() <= 128 * 1024 * 1024,
            "DAT 长度不在支持范围内"
        );
        let rows = pointer(&bytes, 0)?;
        ensure!(rows > 0 && rows <= 250000, "DAT 行数无效");
        let first = pointer(&bytes, 4)? as usize;
        let marker = "Metadata/Items/"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        let marker_at = bytes
            .windows(marker.len())
            .position(|w| w == marker)
            .context("DAT 缺少物品身份标记")?;
        let base = marker_at.checked_sub(first).context("DAT 字符串基址无效")?;
        ensure!(
            base >= 4 && (base - 4) % rows as usize == 0,
            "DAT 行布局存在歧义"
        );
        let row_size = (base - 4) / rows as usize;
        ensure!((36..=4096).contains(&row_size), "DAT 行宽不兼容");
        let layout = DatLayout {
            rows,
            row_size,
            string_base: base,
            name_offset: 32,
        };
        Self::read(bytes, layout, 0, true)
    }
    pub fn words(bytes: Vec<u8>) -> Result<Self> {
        ensure!(
            bytes.len() >= 4 && bytes.len() <= 128 * 1024 * 1024,
            "DAT 长度不在支持范围内"
        );
        let rows = pointer(&bytes, 0)?;
        ensure!(rows > 0 && rows <= 250000, "DAT 行数无效");
        let layout = DatLayout {
            rows,
            row_size: 64,
            string_base: 4 + rows as usize * 64,
            name_offset: 48,
        };
        Self::read(bytes, layout, 4, false)
    }
    fn read(
        bytes: Vec<u8>,
        layout: DatLayout,
        identity_offset: usize,
        metadata: bool,
    ) -> Result<Self> {
        ensure!(layout.string_base < bytes.len(), "DAT 字符串表截断");
        let mut records = Vec::new();
        let mut seen = BTreeSet::new();
        for row in 0..layout.rows {
            let pos = 4 + row as usize * layout.row_size;
            let identity = string(
                &bytes,
                layout.string_base,
                pointer(&bytes, pos + identity_offset)?,
            )?;
            let display = string(
                &bytes,
                layout.string_base,
                pointer(&bytes, pos + layout.name_offset)?,
            )?;
            if metadata {
                ensure!(
                    identity.starts_with("Metadata/Items/") && seen.insert(identity.clone()),
                    "DAT 物品身份无效或重复"
                );
            }
            records.push(DatRecord {
                row,
                identity,
                display,
                name_pointer: pos + layout.name_offset,
            });
        }
        Ok(Self {
            layout,
            records,
            bytes,
        })
    }
    /// Produces new resource bytes only. The caller must separately validate
    /// schema compatibility and hold the target's transaction capability.
    pub fn replace_names(&self, changes: &BTreeMap<u32, String>) -> Result<Vec<u8>> {
        let mut bytes = self.bytes.clone();
        for (row, text) in changes {
            let record = self.records.get(*row as usize).context("DAT 修改行越界")?;
            ensure!(
                !text.contains('\0') && text.chars().count() <= 2048,
                "展示文本包含空字符或过长"
            );
            if &record.display == text {
                continue;
            }
            ensure!(
                bytes.len() % 2 == self.layout.string_base % 2,
                "DAT 字符串表尾部未对齐"
            );
            let relative = u32::try_from(bytes.len() - self.layout.string_base)?;
            let encoded = text
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>();
            ensure!(
                bytes.len() + encoded.len() + 4 <= 128 * 1024 * 1024,
                "DAT 输出超过大小上限"
            );
            bytes.extend(encoded);
            bytes.extend([0u8; 4]);
            bytes[record.name_pointer..record.name_pointer + 4]
                .copy_from_slice(&relative.to_le_bytes());
        }
        Ok(bytes)
    }
}
