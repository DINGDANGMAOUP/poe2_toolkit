//! Pinned DAT64 layouts. A file must match the complete row width and sentinel;
//! unknown revisions are rejected instead of inferring writable offsets.
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{collections::BTreeMap, sync::OnceLock};

#[derive(Debug, Clone, Deserialize)]
pub struct Column {
    pub offset: usize,
    pub size: usize,
    pub kind: String,
}
#[derive(Debug, Clone, Deserialize)]
pub struct Layout {
    pub row_size: usize,
    pub fields: BTreeMap<String, Column>,
}
#[derive(Deserialize)]
struct Registry {
    tables: BTreeMap<String, Layout>,
}

pub struct Table {
    pub rows: usize,
    pub base: usize,
    pub layout: Layout,
    bytes: Vec<u8>,
}

impl Table {
    pub fn parse(name: &str, bytes: Vec<u8>) -> Result<Self> {
        Self::parse_for_game(poe2_core::GameId::Poe2, name, bytes)
    }
    pub fn parse_for_game(game: poe2_core::GameId, name: &str, bytes: Vec<u8>) -> Result<Self> {
        static REGISTRY: OnceLock<Registry> = OnceLock::new();
        let registry = REGISTRY.get_or_init(|| {
            serde_json::from_str(include_str!("../schemas/poe2.json"))
                .expect("embedded DAT registry")
        });
        static POE1: OnceLock<Registry> = OnceLock::new();
        let registry = if game == poe2_core::GameId::Poe1 {
            POE1.get_or_init(|| {
                serde_json::from_str(include_str!("../schemas/poe1.json"))
                    .expect("embedded PoE1 DAT registry")
            })
        } else {
            registry
        };
        let mut layout = registry
            .tables
            .get(name)
            .context("未注册的 DAT 表")?
            .clone();
        ensure!(
            (12..=128 * 1024 * 1024).contains(&bytes.len()),
            "DAT 大小超限"
        );
        let rows = u32::from_le_bytes(bytes[..4].try_into()?) as usize;
        ensure!(rows <= 250_000, "DAT 行数超限");
        // Legacy BaseItemTypes/Stats fields are explicitly documented by the
        // reference reader. No legacy Mods offsets are inferred: an unknown
        // complete layout must be reviewed before it becomes writable.
        if game == poe2_core::GameId::Poe2
            && bytes.get(4 + rows * layout.row_size..4 + rows * layout.row_size + 8)
                != Some(&[0xbb; 8])
        {
            let legacy = match name {
                "BaseItemTypes" => Some(360),

                "Stats" => Some(106),
                _ => None,
            };
            if let Some(width) = legacy
                && bytes.get(4 + rows * width..4 + rows * width + 8) == Some(&[0xbb; 8])
            {
                layout.row_size = width;
                match name {
                    "BaseItemTypes" => layout.fields.retain(|key, _| {
                        matches!(key.as_str(), "Id" | "Name" | "Tags" | "InheritsFrom")
                    }),
                    "Stats" => layout.fields.retain(|key, _| key == "Id"),
                    _ => {}
                }
            }
        }
        let base = 4 + rows * layout.row_size;
        ensure!(
            bytes.get(base..base + 8) == Some(&[0xbb; 8]),
            "{name} 表结构不兼容（要求 {} 字节行和 DAT64 分隔符），未修改资源",
            layout.row_size
        );
        Ok(Self {
            rows,
            base,
            layout,
            bytes,
        })
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn column(&self, name: &str) -> Result<&Column> {
        self.layout
            .fields
            .get(name)
            .context("DAT 字段不在已注册结构内")
    }
    fn at(&self, row: usize, name: &str) -> Result<usize> {
        ensure!(row < self.rows, "DAT 行引用越界");
        Ok(4 + row * self.layout.row_size + self.column(name)?.offset)
    }
    fn u64_at(&self, at: usize) -> Result<u64> {
        Ok(u64::from_le_bytes(
            self.bytes
                .get(at..at + 8)
                .context("DAT 指针截断")?
                .try_into()?,
        ))
    }
    pub fn reference(&self, row: usize, field: &str) -> Result<Option<usize>> {
        let n = self.u64_at(self.at(row, field)?)?;
        if n == u64::MAX || n == 0xfefe_fefe_fefe_fefe {
            return Ok(None);
        }
        Ok(Some(usize::try_from(n)?))
    }
    pub fn integer(&self, row: usize, field: &str) -> Result<i32> {
        let at = self.at(row, field)?;
        ensure!(self.column(field)?.size == 4, "DAT 字段不是整数");
        Ok(i32::from_le_bytes(self.bytes[at..at + 4].try_into()?))
    }
    pub fn interval(&self, row: usize, field: &str) -> Result<(i32, i32)> {
        let at = self.at(row, field)?;
        ensure!(
            self.column(field)?.size == 8 && self.column(field)?.kind == "i32",
            "DAT 字段不是数值区间"
        );
        let low = i32::from_le_bytes(self.bytes[at..at + 4].try_into()?);
        let high = i32::from_le_bytes(self.bytes[at + 4..at + 8].try_into()?);
        ensure!(low <= high, "DAT 数值区间倒置");
        Ok((low, high))
    }
    pub fn text(&self, row: usize, field: &str) -> Result<String> {
        ensure!(self.column(field)?.kind == "string", "DAT 字段不是文本");
        self.string(self.u64_at(self.at(row, field)?)?)
    }
    fn string(&self, relative: u64) -> Result<String> {
        let relative = usize::try_from(relative)?;
        ensure!(
            relative >= 8 && relative.is_multiple_of(2),
            "DAT 文本指针无效"
        );
        let start = self.base.checked_add(relative).context("DAT 指针溢出")?;
        let data = self.bytes.get(start..).context("DAT 文本越界")?;
        let mut units = Vec::new();
        for c in data.as_chunks::<2>().0.iter().take(8193) {
            let u = u16::from_le_bytes(*c);
            if u == 0 {
                return String::from_utf16(&units).context("DAT UTF-16 无效");
            }
            units.push(u);
        }
        anyhow::bail!("DAT 文本未终止或超过上限")
    }
    pub fn array(&self, row: usize, field: &str, stride: usize) -> Result<Vec<u64>> {
        let at = self.at(row, field)?;
        ensure!(
            self.column(field)?.kind.starts_with('[') && matches!(stride, 4 | 8 | 16),
            "DAT 数组字段不兼容"
        );
        let count = usize::try_from(self.u64_at(at)?)?;
        ensure!(count <= 10000, "DAT 数组数量超限");
        if count == 0 {
            return Ok(vec![]);
        }
        let pointer = usize::try_from(self.u64_at(at + 8)?)?;
        ensure!(pointer >= 8, "DAT 数组指针无效");
        let start = self.base.checked_add(pointer).context("DAT 数组溢出")?;
        ensure!(
            start
                .checked_add(count * stride)
                .is_some_and(|end| end <= self.bytes.len()),
            "DAT 数组越界"
        );
        (0..count)
            .map(|i| {
                if stride == 4 {
                    Ok(u32::from_le_bytes(
                        self.bytes[start + i * stride..start + i * stride + 4].try_into()?,
                    ) as u64)
                } else {
                    self.u64_at(start + i * stride)
                }
            })
            .collect()
    }
    pub fn rewrite(&self, changes: &BTreeMap<(usize, String), String>) -> Result<Vec<u8>> {
        let mut out = self.bytes.clone();
        for ((row, field), text) in changes {
            ensure!(
                !text.contains('\0') && text.chars().count() <= 4096,
                "DAT 输出文本过长或包含空字符"
            );
            if self.text(*row, field)? == *text {
                continue;
            }
            let at = self.at(*row, field)?;
            if !(out.len() - self.base).is_multiple_of(2) {
                out.push(0);
            }
            let pointer = (out.len() - self.base) as u64;
            out.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
            out.extend([0; 4]);
            out[at..at + 8].copy_from_slice(&pointer.to_le_bytes());
        }
        ensure!(out.len() <= 128 * 1024 * 1024, "DAT 输出超限");
        Ok(out)
    }
}
