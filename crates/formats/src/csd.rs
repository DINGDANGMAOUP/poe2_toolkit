//! Bounded stat-description editing. Preserve language sections, placeholders,
//! transforms and fallback records. Never infer a stat from translated prose.
use anyhow::{Context, Result, ensure};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub struct Reference {
    pub id: String,
    pub low: i32,
    pub high: i32,
    pub label: String,
}
pub struct Csd {
    lines: Vec<String>,
    utf16: bool,
    bom: bool,
    newline: String,
}
impl Csd {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() <= 16 * 1024 * 1024, "CSD 大小超限");
        let utf16 = bytes.starts_with(&[0xff, 0xfe])
            || bytes
                .iter()
                .take(128)
                .skip(1)
                .step_by(2)
                .filter(|&&b| b == 0)
                .count()
                > 16;
        let bom = if utf16 {
            bytes.starts_with(&[0xff, 0xfe])
        } else {
            bytes.starts_with(&[0xef, 0xbb, 0xbf])
        };
        let text = if utf16 {
            let body = &bytes[if bom { 2 } else { 0 }..];
            ensure!(body.len().is_multiple_of(2), "CSD UTF-16 截断");
            String::from_utf16(
                &body
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes(*c))
                    .collect::<Vec<_>>(),
            )?
        } else {
            String::from_utf8(bytes[if bom { 3 } else { 0 }..].to_vec())?
        };
        ensure!(
            !text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')),
            "CSD 含有非法控制字符"
        );
        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" }.into();
        let lines = text.split_inclusive('\n').map(str::to_owned).collect();
        Ok(Self {
            lines,
            utf16,
            bom,
            newline,
        })
    }
    pub fn annotate(
        &self,
        references: &BTreeMap<String, Vec<Reference>>,
    ) -> Result<(Vec<u8>, BTreeSet<String>)> {
        let mut out = Vec::new();
        let mut used = BTreeSet::new();
        let mut at = 0;
        while at < self.lines.len() {
            if self.lines[at].trim() != "description" {
                out.push(self.lines[at].clone());
                at += 1;
                continue;
            }
            out.push(self.lines[at].clone());
            at += 1;
            let header = self.lines.get(at).context("CSD 缺少属性声明")?;
            let tokens: Vec<_> = header.split_whitespace().collect();
            let n: usize = tokens.first().context("CSD 属性数量缺失")?.parse()?;
            ensure!(
                n > 0 && n <= 32 && tokens.len() == n + 1,
                "CSD 属性声明无效"
            );
            let refs = if n == 1 {
                references.get(tokens[1])
            } else {
                None
            };
            out.push(header.clone());
            at += 1;
            let mut language = String::new();
            while at < self.lines.len() && self.lines[at].trim() != "description" {
                let line = self.lines[at].trim();
                if line.is_empty() || line.starts_with("//") {
                    out.push(self.lines[at].clone());
                    at += 1;
                    continue;
                }
                if line.starts_with("lang ") {
                    language = quoted(line)?.1.to_owned();
                    out.push(self.lines[at].clone());
                    at += 1;
                }
                let count: usize = self
                    .lines
                    .get(at)
                    .context("CSD 缺少语言条目数")?
                    .trim()
                    .parse()
                    .context("CSD 语言条目数无效")?;
                ensure!(
                    count <= 10000 && at + 1 + count <= self.lines.len(),
                    "CSD 条目数量超限或截断"
                );
                at += 1;
                let mut records = Vec::new();
                for line in &self.lines[at..at + count] {
                    let (prefix, text, suffix) = quoted(line)?;
                    let chinese = language.eq_ignore_ascii_case("Simplified Chinese")
                        || language.eq_ignore_ascii_case("Traditional Chinese")
                        || language.is_empty()
                            && text.chars().any(|c| ('\u{3400}'..='\u{9fff}').contains(&c));
                    if chinese
                        && n == 1
                        && (suffix.trim().is_empty() || suffix.trim() == "canonical_line")
                        && let Some(refs) = refs
                    {
                        let (low, high) = bounds(prefix.trim())?;
                        let mut cuts = BTreeSet::<i64>::new();
                        for r in refs {
                            let lo = low.max(r.low);
                            let hi = high.min(r.high);
                            if lo <= hi {
                                cuts.insert(i64::from(lo));
                                cuts.insert(i64::from(hi) + 1);
                            }
                        }
                        let cuts: Vec<_> = cuts.into_iter().collect();
                        for pair in cuts.windows(2) {
                            let lo = pair[0];
                            let hi = pair[1] - 1;
                            let applicable: Vec<_> = refs
                                .iter()
                                .filter(|r| i64::from(r.low) <= lo && i64::from(r.high) >= hi)
                                .collect();
                            if applicable.is_empty() {
                                continue;
                            }
                            let labels: BTreeSet<_> =
                                applicable.iter().map(|r| r.label.as_str()).collect();
                            let label = labels.into_iter().collect::<Vec<_>>().join("；");
                            ensure!(
                                label.len() <= 2048 && !label.contains(['"', '\r', '\n', '\\']),
                                "CSD 价格文本不安全或过长"
                            );
                            let condition = if lo == hi {
                                lo.to_string()
                            } else {
                                format!("{lo}|{hi}")
                            };
                            records.push(format!(
                                "\t{condition} \"{text}（条件参考：{label}）\"{}{}",
                                suffix.trim_end_matches(['\r', '\n']),
                                self.newline
                            ));
                            for r in applicable {
                                used.insert(r.id.clone());
                            }
                        }
                    }
                    records.push(line.clone());
                }
                out.push(format!("\t{}{}", records.len(), self.newline));
                out.extend(records);
                at += count;
            }
        }
        let text = out.concat();
        let mut bytes = if self.bom {
            if self.utf16 {
                vec![0xff, 0xfe]
            } else {
                vec![0xef, 0xbb, 0xbf]
            }
        } else {
            vec![]
        };
        if self.utf16 {
            bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        } else {
            bytes.extend(text.as_bytes());
        }
        Ok((bytes, used))
    }
}
fn quoted(line: &str) -> Result<(&str, &str, &str)> {
    let start = line.find('"').context("CSD 条目缺少引号")?;
    let mut escaped = false;
    let mut end = None;
    for (n, c) in line[start + 1..].char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            end = Some(start + 1 + n);
            break;
        }
    }
    let end = end.context("CSD 引号未闭合")?;
    Ok((&line[..start], &line[start + 1..end], &line[end + 1..]))
}
fn bounds(text: &str) -> Result<(i32, i32)> {
    if text == "#" {
        return Ok((i32::MIN, i32::MAX));
    }
    if let Some((lo, hi)) = text.split_once('|') {
        let low = if lo == "#" { i32::MIN } else { lo.parse()? };
        let high = if hi == "#" { i32::MAX } else { hi.parse()? };
        ensure!(low <= high, "CSD 数值区间倒置");
        Ok((low, high))
    } else {
        let n = text.parse()?;
        Ok((n, n))
    }
}
