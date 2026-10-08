use super::*;
use crate::csd::{Csd, Reference};

impl Planner<'_> {
    pub(super) fn affixes(&mut self) -> Result<()> {
        if !self.enabled(Feature::TabletAffixes) {
            return Ok(());
        }
        let path = "data/statdescriptions/tablet_stat_descriptions.csd";
        if !self.loose.has(path)? {
            self.warnings
                .insert("缺少碑牌 stat descriptions，本层未修改".into());
            return Ok(());
        }
        let candidates: Vec<_> = self
            .snapshot
            .into_iter()
            .flat_map(|s| &s.quotes)
            .filter(|q| q.feature() == Feature::TabletAffixes && self.rules.permits(q, Utc::now()))
            .collect();
        if candidates.is_empty() {
            if let Some(owned) = &self.loose.owned
                && owned.scope == self.profile.market
            {
                let fields: Vec<_> = owned
                    .fields
                    .iter()
                    .filter(|f| f.feature == Feature::TabletAffixes && self.retained(f))
                    .cloned()
                    .collect();
                if !fields.is_empty()
                    && fields.len()
                        == owned
                            .fields
                            .iter()
                            .filter(|f| f.feature == Feature::TabletAffixes)
                            .count()
                {
                    self.replacements.insert(
                        path.into(),
                        crate::bundle::extract_resource(
                            &self.profile.installation.as_ref().unwrap().root,
                            path,
                        )?,
                    );
                    self.desired.extend(fields);
                    self.warnings
                        .insert("碑牌词缀行情不可用，保留上次词缀标注".into());
                }
            }
            return Ok(());
        }
        let bases = self.table(ENGLISH, "BaseItemTypes")?;
        let local_bases = self.table(LOCAL, "BaseItemTypes")?;
        let mods = self.table(ENGLISH, "Mods")?;
        let stats = self.table(ENGLISH, "Stats")?;
        let tags = self.table(ENGLISH, "Tags")?;
        let mut pools = BTreeMap::<String, (String, BTreeSet<String>)>::new();
        for row in 0..bases.rows {
            let id = bases.text(row, "Id")?;
            let Some(slug) = tablet_slug(&id) else {
                continue;
            };
            ensure!(local_bases.text(row, "Id")? == id, "碑牌中英文身份不一致");
            let mut allowed = BTreeSet::from(["default".into()]);
            for n in bases.array(row, "Tags", 16)? {
                allowed.insert(tags.text(usize::try_from(n)?, "Id")?);
            }
            ensure!(
                pools
                    .insert(slug.to_owned(), (local_bases.text(row, "Name")?, allowed))
                    .is_none(),
                "碑牌底材身份重复"
            );
        }
        let mut references = BTreeMap::<String, Vec<Reference>>::new();
        let mut descriptions = BTreeMap::new();
        for q in candidates {
            let Some(TabletIdentity::Affix {
                base,
                rarity,
                modifier,
                generation,
                query,
                ..
            }) = &q.tablet
            else {
                continue;
            };
            let Some((base_name, allowed)) = pools.get(base) else {
                continue;
            };
            let exact_query = query
                .get("stats")
                .and_then(|v| v.as_array())
                .is_some_and(|groups| {
                    groups
                        .iter()
                        .filter(|g| {
                            g.get("filters")
                                .and_then(|f| f.as_array())
                                .is_some_and(|filters| {
                                    filters.iter().any(|f| {
                                        f["id"]
                                            .as_str()
                                            .is_some_and(|id| id.starts_with("explicit."))
                                    })
                                })
                        })
                        .all(|g| {
                            g["type"] == "and"
                                && g["filters"].as_array().is_some_and(|f| f.len() == 1)
                        })
                });
            if !exact_query {
                self.warnings.insert(format!(
                    "{base}/{modifier} 的交易查询合并多个属性，本次不将其当作单词缀价格"
                ));
                continue;
            }
            let generation_code = if generation == "prefix" { 1 } else { 2 };
            let mut matches = Vec::new();
            for row in 0..mods.rows {
                if mods.integer(row, "Domain")? != 34
                    || mods.integer(row, "GenerationType")? != generation_code
                {
                    continue;
                }
                let id = mods.text(row, "Id")?;
                if &id != modifier && mods.text(row, "Name")? != *modifier {
                    continue;
                }
                let tag_refs = mods.array(row, "SpawnWeight_Tags", 16)?;
                let weights = mods.array(row, "SpawnWeight_Values", 4)?;
                ensure!(
                    tag_refs.len() == weights.len(),
                    "词缀权重与 Tags 数量不一致"
                );
                let mut weight = 0;
                for (t, w) in tag_refs.iter().zip(weights) {
                    if allowed.contains(&tags.text(usize::try_from(*t)?, "Id")?) {
                        weight = w as u32 as i32;
                        break;
                    }
                }
                if weight <= 0 {
                    continue;
                }
                // Multi-stat modifiers need a full tuple contract; never price
                // just their first stat as though it represented the whole mod.
                if [
                    "Stat2", "Stat3", "Stat4", "Stat5", "Stat6", "Stat7", "Stat8",
                ]
                .iter()
                .map(|f| mods.reference(row, f))
                .collect::<Result<Vec<_>>>()?
                .iter()
                .any(Option::is_some)
                {
                    continue;
                }
                let stat = stats.text(
                    mods.reference(row, "Stat1")?.context("词缀缺少 Stat1")?,
                    "Id",
                )?;
                matches.push((id, stat, mods.interval(row, "Stat1Value")?));
            }
            if matches.len() != 1 {
                self.warnings.insert(format!(
                    "碑牌词缀 {base}/{modifier} 没有唯一可生成的单属性关联，已跳过"
                ));
                continue;
            }
            let (id, stat, (low, high)) = matches.remove(0);
            // Query restrictions remain visible; a single modifier price is
            // never presented as the price of an entire tablet.
            let minimum = minimum_uses(query, base).context("碑牌词缀查询缺少剩余次数约束")?;
            let label = format!(
                "{base_name}·{}·≥{minimum}次 {}",
                rarity.label(),
                self.rules.price(q)
            );
            let key = format!("{base}/{id}/{}", rarity.label());
            if descriptions.insert(key.clone(), label.clone()).is_some() {
                bail!("碑牌词缀报价身份冲突：{key}");
            }
            references.entry(stat).or_default().push(Reference {
                id: q.item_id.clone(),
                low,
                high,
                label,
            });
        }
        let source = self.loose.resource(path)?;
        let (output, used) = Csd::parse(&source)?.annotate(&references)?;
        if used.is_empty() {
            self.warnings
                .insert("碑牌 CSD 没有与词缀范围兼容的中文描述分支".into());
            return Ok(());
        }
        // A second structural parse validates generated language record counts.
        Csd::parse(&output)?.annotate(&BTreeMap::new())?;
        for (stat, refs) in &references {
            let labels: Vec<_> = refs
                .iter()
                .filter(|r| used.contains(&r.id))
                .map(|r| r.label.clone())
                .collect();
            if !labels.is_empty() {
                self.record(path,stat,Feature::TabletAffixes,stat,&labels.join("；"),"Mods/Tags/Stats 精确关联，CSD 按数值区间分支；每种底材和稀有度单独显示条件参考");
                let quotes: Vec<_> = self
                    .snapshot
                    .into_iter()
                    .flat_map(|s| s.quotes.iter())
                    .filter(|q| {
                        refs.iter()
                            .any(|r| r.id == q.item_id && used.contains(&r.id))
                    })
                    .collect();
                self.attach_evidence(path, stat, &quotes);
            }
        }
        self.quotes.extend(used);
        self.replacements.insert(path.into(), output);
        Ok(())
    }
}
fn minimum_uses(value: &serde_json::Value, base: &str) -> Option<u64> {
    let stat = match base {
        "Abyss_Tablet" => "2369421690",
        "Breach_Tablet" => "2219129443",
        "Delirium_Tablet" => "3879011313",
        "Expedition_Tablet" => "1714888636",
        "Irradiated_Tablet" => "4041853756",
        "Overseer_Tablet" => "3376302538",
        "Ritual_Tablet" => "3166002380",
        "Temple_Tablet" => "3035440454",
        _ => return None,
    };
    let expected = format!("implicit.stat_{stat}");
    let matches: Vec<_> = value
        .get("stats")?
        .as_array()?
        .iter()
        .flat_map(|group| {
            group
                .get("filters")
                .and_then(|f| f.as_array())
                .into_iter()
                .flatten()
        })
        .filter(|f| f.get("id").and_then(|id| id.as_str()) == Some(expected.as_str()))
        .collect();
    if matches.len() != 1 {
        return None;
    }
    matches[0].get("value")?.get("min")?.as_u64()
}
