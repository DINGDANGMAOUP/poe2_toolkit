//! Bundles2 resource storage, loose or inside a GGPK container.
use crate::{
    bundle::{self, BundleIndex},
    checked_path, hash_file,
};
use anyhow::{Context, Result, ensure};
use poe2_core::{FieldChange, FileExpectation, FileMutation, MarketScope, digest};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub const OWNERSHIP: &str = ".poe2-toolkit-ownership.json";
const OVERLAY: &str = "poe2-toolkit-overlay";

#[derive(Serialize, Deserialize)]
pub(crate) struct Ownership {
    version: u32,
    index_path: String,
    baseline_index: Vec<u8>,
    output_index_hash: String,
    output_bundle_hash: String,
    sources: BTreeMap<String, String>,
    pub scope: Option<MarketScope>,
    pub fields: Vec<FieldChange>,
    #[serde(default)]
    container_baseline: Option<crate::ggpk::Baseline>,
}

pub(crate) struct BundleStore {
    container: Option<crate::ggpk::Ggpk>,
    container_source_hash: Option<String>,
    root: PathBuf,
    pub index_path: String,
    pub prefix: String,
    index_bytes: Vec<u8>,
    pub index: BundleIndex,
    reads: BTreeMap<String, Option<String>>,
    sources: BTreeMap<String, String>,
    cache: BTreeMap<u32, Vec<u8>>,
    pub owned: Option<Ownership>,
}

fn hash_if_present(root: &Path, path: &str) -> Result<Option<String>> {
    let path = checked_path(root, path)?;
    if path.exists() {
        Ok(Some(hash_file(&path)?))
    } else {
        Ok(None)
    }
}

impl BundleStore {
    pub fn open(root: &Path) -> Result<Self> {
        let container = if !checked_path(root, "Bundles2/_.index.bin")?.is_file()
            && !checked_path(root, "_.index.bin")?.is_file()
        {
            Some(crate::ggpk::Ggpk::open(root)?)
        } else {
            None
        };
        let read = |path: &str| -> Result<Vec<u8>> {
            if let Some(c) = &container {
                c.read(path)?
                    .with_context(|| format!("GGPK 缺少资源 {path}"))
            } else {
                bundle::read_bounded(&checked_path(root, path)?)
            }
        };
        let container_source_hash = container.as_ref().map(|c| hash_file(&c.path)).transpose()?;
        let hash = |path: &str| -> Result<Option<String>> {
            if let Some(c) = &container {
                Ok(c.read(path)?.map(|b| digest(&b)))
            } else {
                hash_if_present(root, path)
            }
        };
        let prefix =
            if container.is_some() || checked_path(root, "Bundles2/_.index.bin")?.is_file() {
                "Bundles2/"
            } else {
                ""
            }
            .to_owned();
        let index_path = format!("{prefix}_.index.bin");
        let current_index = read(&index_path)?;
        let overlay_path = format!("{prefix}{OVERLAY}.bundle.bin");
        let ownership_path = checked_path(root, OWNERSHIP)?;
        let owned: Option<Ownership> = if ownership_path.exists() {
            ensure!(
                std::fs::metadata(&ownership_path)?.len() <= 128 * 1024 * 1024,
                "补丁归属记录过大"
            );
            Some(serde_json::from_slice(&std::fs::read(&ownership_path)?)?)
        } else {
            None
        };
        let mut sources = BTreeMap::new();
        let index_bytes = if let Some(o) = &owned {
            ensure!(
                o.version == 1 && o.index_path == index_path,
                "补丁归属记录不兼容"
            );
            ensure!(
                o.output_index_hash == digest(&current_index),
                "游戏索引已被更新或其他工具修改；请先检查历史记录，禁止沿用旧基线"
            );
            ensure!(
                hash(&overlay_path)? == Some(o.output_bundle_hash.clone()),
                "补丁资源包已被外部修改"
            );
            ensure!(
                o.sources.len() <= 512 && o.fields.len() <= 100_000,
                "补丁归属记录超限"
            );
            for (path, expected) in &o.sources {
                ensure!(
                    hash(path)? == Some(expected.clone()),
                    "原始资源被外部修改：{path}"
                );
            }
            sources.clone_from(&o.sources);
            o.baseline_index.clone()
        } else {
            ensure!(
                hash(&overlay_path)?.is_none(),
                "补丁包同名文件已存在且没有归属记录，拒绝覆盖"
            );
            current_index.clone()
        };
        let index = BundleIndex::parse(bundle::decode(&index_bytes)?)?;
        ensure!(
            !index
                .bundles
                .iter()
                .any(|b| b.name.eq_ignore_ascii_case(OVERLAY)),
            "基线索引含有未恢复的 Toolkit 资源引用"
        );
        let mut reads = BTreeMap::from([(OWNERSHIP.into(), hash_if_present(root, OWNERSHIP)?)]);
        if container.is_none() {
            reads.insert(index_path.clone(), Some(digest(&current_index)));
            reads.insert(overlay_path.clone(), hash(&overlay_path)?);
        }
        Ok(Self {
            container,
            container_source_hash,
            root: root.into(),
            index_path,
            prefix,
            index_bytes,
            index,
            reads,
            sources,
            cache: BTreeMap::new(),
            owned,
        })
    }
    pub fn has(&self, path: &str) -> Result<bool> {
        Ok(self.index.lookup(path)?.is_some())
    }
    pub fn resource(&mut self, path: &str) -> Result<Vec<u8>> {
        let entry = self
            .index
            .lookup(path)?
            .with_context(|| format!("缺少资源 {path}"))?;
        let bundle = &self.index.bundles[entry.bundle as usize];
        if !self.cache.contains_key(&entry.bundle) {
            let relative = format!("{}{}.bundle.bin", self.prefix, bundle.name);
            let raw = if let Some(c) = &self.container {
                c.read(&relative)?.context("GGPK 中缺少源 Bundle")?
            } else {
                bundle::read_bounded(&checked_path(&self.root, &relative)?)?
            };
            self.sources.insert(relative.clone(), digest(&raw));
            if self.container.is_none() {
                self.reads.insert(relative, Some(digest(&raw)));
            }
            let bytes = bundle::decode(&raw)?;
            ensure!(
                bytes.len() == bundle.uncompressed_size as usize,
                "Bundle 与索引长度不一致"
            );
            ensure!(
                self.cache.values().map(Vec::len).sum::<usize>() + bytes.len() <= 768 * 1024 * 1024,
                "本次资源读取超出内存预算"
            );
            self.cache.insert(entry.bundle, bytes);
        }
        Ok(self.cache[&entry.bundle]
            [entry.offset as usize..entry.offset as usize + entry.size as usize]
            .to_vec())
    }
    pub fn materialize(
        mut self,
        replacements: BTreeMap<String, Vec<u8>>,
        fields: Vec<FieldChange>,
        scope: Option<MarketScope>,
    ) -> Result<(Vec<FileMutation>, Vec<FileExpectation>)> {
        let overlay_path = format!("{}{OVERLAY}.bundle.bin", self.prefix);
        let mut mutations = Vec::new();
        if replacements.is_empty() {
            if self.owned.is_some() {
                if let Some(container) = &self.container {
                    let baseline = self
                        .owned
                        .as_ref()
                        .and_then(|o| o.container_baseline.as_ref())
                        .context("缺少 GGPK 原始基线")?;
                    let (recipe, hash) = container.restore(baseline)?;
                    self.container_mutation(&mut mutations, recipe, hash)?;
                } else {
                    self.mutation(
                        &mut mutations,
                        self.index_path.clone(),
                        Some(self.index_bytes.clone()),
                    )?;
                    self.mutation(&mut mutations, overlay_path, None)?;
                }
                self.mutation(&mut mutations, OWNERSHIP.into(), None)?;
            }
        } else {
            let (raw, payload) = self.index.replace_existing(OVERLAY, &replacements)?;
            let index_bytes = bundle::encode_raw(&raw)?;
            let bundle_bytes = bundle::encode_raw(&payload)?;
            let verify_index = BundleIndex::parse(bundle::decode(&index_bytes)?)?;
            let verify_payload = bundle::decode(&bundle_bytes)?;
            for (path, expected) in &replacements {
                let e = verify_index.lookup(path)?.context("输出索引丢失资源")?;
                ensure!(
                    &verify_payload[e.offset as usize..e.offset as usize + e.size as usize]
                        == expected,
                    "输出资源校验失败"
                );
            }
            let ownership = Ownership {
                version: 1,
                index_path: self.index_path.clone(),
                baseline_index: self.index_bytes.clone(),
                output_index_hash: digest(&index_bytes),
                output_bundle_hash: digest(&bundle_bytes),
                sources: self.sources.clone(),
                scope,
                fields,
                container_baseline: if let Some(c) = &self.container {
                    Some(if let Some(o) = &self.owned {
                        o.container_baseline.clone().context("缺少容器基线")?
                    } else {
                        c.baseline()?
                    })
                } else {
                    None
                },
            };
            if let Some(container) = &self.container {
                let unchanged = self.owned.as_ref().is_some_and(|o| {
                    o.output_index_hash == ownership.output_index_hash
                        && o.output_bundle_hash == ownership.output_bundle_hash
                });
                if !unchanged {
                    let (recipe, hash) = container.rewrite(&BTreeMap::from([
                        (overlay_path, bundle_bytes),
                        (self.index_path.clone(), index_bytes),
                    ]))?;
                    self.container_mutation(&mut mutations, recipe, hash)?;
                }
            } else {
                self.mutation(&mut mutations, overlay_path, Some(bundle_bytes))?;
                self.mutation(&mut mutations, self.index_path.clone(), Some(index_bytes))?;
            }
            self.mutation(
                &mut mutations,
                OWNERSHIP.into(),
                Some(serde_json::to_vec(&ownership)?),
            )?;
        }
        if self.container.is_none() {
            self.reads.extend(
                self.sources
                    .into_iter()
                    .map(|(path, hash)| (path, Some(hash))),
            );
        } else if let Some(c) = &self.container {
            ensure!(
                Some(hash_file(&c.path)?) == self.container_source_hash,
                "GGPK 在规划期间发生变化，请重新预览"
            );
            self.reads
                .insert(c.relative.clone(), self.container_source_hash.clone());
        }
        Ok((
            mutations,
            self.reads
                .into_iter()
                .map(|(relative_path, hash)| FileExpectation {
                    relative_path,
                    hash,
                })
                .collect(),
        ))
    }
    fn container_mutation(
        &mut self,
        out: &mut Vec<FileMutation>,
        recipe: poe2_core::StreamRecipe,
        after_hash: String,
    ) -> Result<()> {
        ensure!(
            Some(&recipe.source_hash) == self.container_source_hash.as_ref(),
            "GGPK 在规划期间发生变化"
        );
        let path = self
            .container
            .as_ref()
            .context("没有 GGPK 后端")?
            .relative
            .clone();
        self.reads
            .insert(path.clone(), Some(recipe.source_hash.clone()));
        out.push(FileMutation {
            relative_path: path,
            before_hash: Some(recipe.source_hash.clone()),
            after_hash,
            content: vec![],
            remove: false,
            rebuild: Some(recipe),
        });
        Ok(())
    }
    fn mutation(
        &self,
        out: &mut Vec<FileMutation>,
        path: String,
        content: Option<Vec<u8>>,
    ) -> Result<()> {
        let before_hash = hash_if_present(&self.root, &path)?;
        let remove = content.is_none();
        let bytes = content.unwrap_or_default();
        let after_hash = digest(&bytes);
        if remove && before_hash.is_none() || !remove && before_hash.as_ref() == Some(&after_hash) {
            return Ok(());
        }
        out.push(FileMutation {
            relative_path: path,
            before_hash,
            after_hash,
            content: bytes,
            remove,
            rebuild: None,
        });
        Ok(())
    }
}
