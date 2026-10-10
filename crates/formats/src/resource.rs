//! Deterministic semantic planning over ordinary game resources.
use crate::{bundle_store::BundleStore, schema::Table};
use anyhow::{Context, Result, bail, ensure};
use chrono::Utc;
use poe2_core::*;
use std::collections::{BTreeMap, BTreeSet};

#[path = "affixes.rs"]
mod affixes;

const LOCAL: &str = "data/balance/simplified chinese";
const ENGLISH: &str = "data/balance";

struct Planner<'a> {
    profile: &'a Profile,
    snapshot: Option<&'a MarketSnapshot>,
    rules: &'a AnnotationRules,
    loose: BundleStore,
    desired: Vec<FieldChange>,
    quotes: BTreeSet<String>,
    warnings: BTreeSet<String>,
    replacements: BTreeMap<String, Vec<u8>>,
}
impl Planner<'_> {
    fn local(&self) -> &'static str {
        if self.profile.game == GameId::Poe1 {
            "data/simplified chinese"
        } else {
            LOCAL
        }
    }
    fn english(&self) -> &'static str {
        if self.profile.game == GameId::Poe1 {
            "data"
        } else {
            ENGLISH
        }
    }
    fn retained(&self, old: &FieldChange) -> bool {
        old.price_evidence
            .as_ref()
            .is_some_and(|e| e.can_retain(Utc::now(), self.profile.annotation.retention_minutes))
    }
    fn enabled(&self, feature: Feature) -> bool {
        self.profile.features.contains(&feature)
    }
    fn previous(&self, resource: &str, identity: &str, feature: Feature) -> Option<&FieldChange> {
        self.loose
            .owned
            .as_ref()?
            .fields
            .iter()
            .find(|c| c.resource == resource && c.record_id == identity && c.feature == feature)
    }
    fn keep_or_price(
        &mut self,
        resource: &str,
        identity: &str,
        feature: Feature,
        before: &str,
        candidates: Vec<&Quote>,
    ) -> String {
        if !self.enabled(feature) {
            return before.into();
        }
        let valid: Vec<_> = candidates
            .into_iter()
            .filter(|q| {
                self.rules.permits(q, Utc::now())
                    && (self.profile.annotation.allow_international_reference
                        || self.snapshot.is_none_or(|s| !q.is_reference_for(&s.scope)))
            })
            .collect();
        if feature == Feature::TabletNames
            && !valid.is_empty()
            && valid.iter().all(|q| {
                matches!(
                    q.tablet,
                    Some(TabletIdentity::Name {
                        rarity: TabletRarity::Normal | TabletRarity::Magic | TabletRarity::Rare,
                        ..
                    })
                )
            })
        {
            let mut scopes = BTreeSet::new();
            let unique = valid.iter().all(|q| {
                if let Some(TabletIdentity::Name {
                    base,
                    rarity,
                    minimum_uses,
                }) = &q.tablet
                {
                    scopes.insert((base, rarity.label(), minimum_uses))
                } else {
                    false
                }
            });
            if unique {
                let labels = valid
                    .iter()
                    .map(|q| {
                        self.quotes.insert(q.item_id.clone());
                        if let Some(TabletIdentity::Name {
                            rarity,
                            minimum_uses,
                            ..
                        }) = &q.tablet
                        {
                            format!(
                                "{}≥{}次 {}",
                                rarity.label(),
                                minimum_uses,
                                self.rules.price(q)
                            )
                        } else {
                            unreachable!()
                        }
                    })
                    .collect::<Vec<_>>();
                let after = format!("{before} [{}]", labels.join(" / "));
                self.record(
                    resource,
                    identity,
                    feature,
                    before,
                    &after,
                    "同底材按稀有度与剩余次数分别显示，保留原币种",
                );
                self.attach_evidence(resource, identity, &valid);
                return after;
            }
        }
        if valid.len() == 1 {
            let q = valid[0];
            self.quotes.insert(q.item_id.clone());
            let condition = match &q.tablet {
                Some(TabletIdentity::Name {
                    rarity,
                    minimum_uses,
                    ..
                }) => format!("{} ≥{}次 · ", rarity.label(), minimum_uses),
                _ => String::new(),
            };
            let reference = self.snapshot.is_some_and(|s| q.is_reference_for(&s.scope));
            let marker = if reference { "国际参考 · " } else { "" };
            let detail = if self.profile.annotation.detailed {
                format!(" · {}", q.statistic.label())
            } else {
                String::new()
            };
            let after = format!(
                "{before} [{marker}{}{}{detail}]",
                condition,
                self.rules.price(q)
            );
            let reason = if reference {
                format!(
                    "{} · {}；唯一资源身份关联，保留来源原币种",
                    q.provider,
                    q.market_label(&self.snapshot.unwrap().scope)
                )
            } else {
                "唯一资源身份关联；保留原币种与查询条件".into()
            };
            self.record(resource, identity, feature, before, &after, &reason);
            self.attach_evidence(resource, identity, &[q]);
            return after;
        }
        if valid.len() > 1 {
            self.warnings.insert(format!(
                "{}：{identity} 对应多个报价，拒绝混合变体",
                feature.label()
            ));
        }
        if let Some(old) = self.previous(resource, identity, feature).cloned() {
            if self.loose.owned.as_ref().and_then(|o| o.scope.as_ref())
                == self.profile.market.as_ref()
                && self.retained(&old)
            {
                self.warnings.insert(format!(
                    "{}部分报价不可用，保留上次标注及其原始价格",
                    feature.label()
                ));
                self.desired.push(old.clone());
                return old.after;
            }
            self.warnings.insert(format!(
                "{}旧价已到期或市场改变，本次清除旧标注",
                feature.label()
            ));
        }
        before.into()
    }
    fn record(
        &mut self,
        resource: &str,
        identity: &str,
        feature: Feature,
        before: &str,
        after: &str,
        reason: &str,
    ) {
        if before != after {
            self.desired.push(FieldChange {
                price_evidence: None,
                resource: resource.into(),
                record_id: identity.into(),
                feature,
                before: before.into(),
                after: after.into(),
                reason: reason.into(),
            });
        }
    }
    fn attach_evidence(&mut self, resource: &str, identity: &str, quotes: &[&Quote]) {
        let Some(snapshot) = self.snapshot else {
            return;
        };
        let Some(fetched_at) = quotes.iter().map(|q| q.fetched_at).min() else {
            return;
        };
        let observed_at = if quotes.iter().all(|q| q.observed_at.is_some()) {
            quotes.iter().filter_map(|q| q.observed_at).min()
        } else {
            None
        };
        let anchor = quotes
            .iter()
            .map(|q| q.observed_at.unwrap_or(q.fetched_at).min(q.fetched_at))
            .min()
            .unwrap();
        if let Some(field) = self
            .desired
            .iter_mut()
            .rev()
            .find(|f| f.resource == resource && f.record_id == identity)
        {
            field.price_evidence = Some(AnnotationEvidence {
                quote_ids: quotes.iter().map(|q| q.item_id.clone()).collect(),
                snapshot_id: snapshot.id.clone(),
                observed_at,
                fetched_at,
                expires_at: anchor
                    + chrono::Duration::minutes(i64::from(self.rules.max_age_minutes)),
            });
        }
    }
    fn table(&mut self, directory: &str, name: &str) -> Result<Table> {
        Table::parse_for_game(
            self.profile.game,
            name,
            self.loose
                .resource(&format!("{directory}/{}.datc64", name.to_lowercase()))?,
        )
    }
    fn names(&mut self, words: bool) -> Result<()> {
        let table_name = if words { "Words" } else { "BaseItemTypes" };
        let file = format!("{}/{}.datc64", self.local(), table_name.to_lowercase());
        let relevant = if words {
            vec![Feature::Unique, Feature::TabletNames]
        } else {
            vec![
                Feature::Currency,
                Feature::TabletNames,
                Feature::DivinationCards,
                Feature::Materials,
            ]
        };
        if !relevant.iter().any(|f| self.enabled(*f)) {
            return Ok(());
        }
        if !self.loose.has(&file)? {
            self.warnings
                .insert(format!("缺少简体中文 {table_name}，未修改英文筛选器身份表"));
            return Ok(());
        }
        let localized = self.table(self.local(), table_name)?;
        let english = self.table(self.english(), table_name)?;
        ensure!(
            localized.rows == english.rows,
            "{table_name} 中英文行数不同，拒绝猜测资源关联"
        );
        let id_field = if words { "Text" } else { "Id" };
        let text_field = if words { "Text2" } else { "Name" };
        let mut identities = BTreeSet::new();
        let mut names = BTreeMap::<String, usize>::new();
        for row in 0..english.rows {
            let identity = english.text(row, id_field)?;
            ensure!(
                identities.insert(identity.clone()),
                "{table_name} 身份重复，无法唯一关联"
            );
            ensure!(
                localized.text(row, id_field)? == identity,
                "{table_name} 中英文身份顺序不一致"
            );
            for name in BTreeSet::from([
                english.text(row, text_field)?,
                localized.text(row, text_field)?,
                identity,
            ]) {
                *names.entry(name).or_default() += 1;
            }
        }
        let snapshot = self.snapshot;
        let mut changes = BTreeMap::new();
        for row in 0..english.rows {
            let identity = english.text(row, id_field)?;
            // Game tables contain both Metadata/Items/ and Metadata/items/.
            // Fold only for namespace checks; preserve the exact record identity.
            let item_path = identity.to_ascii_lowercase();
            if !words {
                ensure!(
                    item_path.starts_with("metadata/items/"),
                    "BaseItemTypes 第 {} 行的物品路径无效：{identity}",
                    row + 1
                );
            }
            let before = localized.text(row, text_field)?;
            let english_name = english.text(row, text_field)?;
            let is_tablet = if words {
                snapshot.is_some_and(|s| {
                    s.quotes.iter().any(|q| {
                        q.feature() == Feature::TabletNames
                            && quote_name_matches(q, &identity, &english_name, &before, &names)
                    })
                }) || self
                    .previous(&file, &identity, Feature::TabletNames)
                    .is_some()
            } else {
                item_path.starts_with("metadata/items/toweraugment/")
            };
            let feature = if is_tablet {
                Feature::TabletNames
            } else if words {
                Feature::Unique
            } else if self.profile.game == GameId::Poe1 {
                let mapped: BTreeSet<_> = snapshot
                    .into_iter()
                    .flat_map(|s| s.quotes.iter())
                    .filter(|q| quote_name_matches(q, &identity, &english_name, &before, &names))
                    .map(Quote::feature)
                    .filter(|f| {
                        matches!(
                            f,
                            Feature::Currency | Feature::DivinationCards | Feature::Materials
                        )
                    })
                    .collect();
                if mapped.len() == 1 {
                    *mapped.first().unwrap()
                } else if let Some(previous) = self.loose.owned.as_ref().and_then(|o| {
                    o.fields
                        .iter()
                        .find(|f| f.record_id == identity && f.resource == file)
                }) {
                    previous.feature
                } else {
                    continue;
                }
            } else {
                Feature::Currency
            };
            if self.profile.game == GameId::Poe2
                && !words
                && !is_tablet
                && !item_path.starts_with("metadata/items/currency/")
            {
                continue;
            }
            let candidates = snapshot
                .into_iter()
                .flat_map(|s| s.quotes.iter())
                .filter(|q| {
                    q.feature() == feature
                        && (!words
                            || matches!(
                                q.tablet,
                                Some(TabletIdentity::Name {
                                    rarity: TabletRarity::Unique,
                                    ..
                                }) | None
                            ))
                        && (words
                            || !matches!(
                                q.tablet,
                                Some(TabletIdentity::Name {
                                    rarity: TabletRarity::Unique,
                                    ..
                                })
                            ))
                        && quote_name_matches(q, &identity, &english_name, &before, &names)
                })
                .collect();
            let after = self.keep_or_price(&file, &identity, feature, &before, candidates);
            if after != before {
                changes.insert((row, text_field.into()), after);
            }
        }
        if !changes.is_empty() {
            let bytes = localized.rewrite(&changes)?;
            let verify = Table::parse_for_game(self.profile.game, table_name, bytes.clone())?;
            for ((row, field), text) in changes {
                ensure!(verify.text(row, &field)? == text, "DAT 名称读回不一致");
            }
            self.replacements.insert(file, bytes);
        }
        Ok(())
    }
    fn islands(&mut self) -> Result<()> {
        if !self.enabled(Feature::MapHints) {
            return Ok(());
        }
        let path = format!("{LOCAL}/endgamemaps.datc64");
        if !self.loose.has(&path)? {
            self.warnings
                .insert("缺少 EndgameMaps 资源，岛屿提示未适配".into());
            return Ok(());
        }
        let maps = self.table(LOCAL, "EndgameMaps")?;
        let areas = self.table(ENGLISH, "WorldAreas")?;
        let mut changes = BTreeMap::new();
        let mut ids = BTreeSet::new();
        for row in 0..maps.rows {
            let area_row = maps
                .reference(row, "WorldArea")?
                .context("EndgameMaps 缺少区域引用")?;
            let identity = areas.text(area_row, "Id")?;
            ensure!(ids.insert(identity.clone()), "EndgameMaps 区域身份重复");
            let name = areas.text(area_row, "Name")?;
            let candidates: Vec<_> = self
                .rules
                .islands
                .iter()
                .filter(|r| {
                    r.id == format!("island:{identity}") || r.id == format!("island:{name}")
                })
                .collect();
            if candidates.len() != 1 {
                continue;
            }
            // Name-based catalogue keys are admitted only if the current client
            // resolves them to exactly one stable WorldAreas identity.
            let mut count = 0;
            for n in 0..areas.rows {
                if areas.text(n, "Name")? == name {
                    count += 1;
                }
            }
            ensure!(count == 1, "岛屿目录名称不能唯一关联 WorldAreas");
            let before = maps.text(row, "FlavourText")?;
            let after = format!("{} [{}]", before, candidates[0].text());
            self.record(
                &path,
                &identity,
                Feature::MapHints,
                &before,
                &after,
                "WorldArea 引用关联；保留原传言",
            );
            changes.insert((row, "FlavourText".into()), after);
        }
        if changes.is_empty() {
            self.warnings
                .insert("当前 EndgameMaps 没有唯一匹配的岛屿目录项".into());
        } else {
            self.replacements.insert(path, maps.rewrite(&changes)?);
        }
        Ok(())
    }
}

fn quote_name_matches(
    q: &Quote,
    identity: &str,
    english: &str,
    localized: &str,
    counts: &BTreeMap<String, usize>,
) -> bool {
    if let Some(TabletIdentity::Name { base, rarity, .. }) = &q.tablet
        && *rarity != TabletRarity::Unique
        && tablet_slug(identity) == Some(base.as_str())
    {
        return true;
    }
    if let Some(metadata) = &q.metadata_id {
        return metadata == identity;
    }
    let name = match &q.tablet {
        Some(TabletIdentity::Name { base, rarity, .. }) if *rarity != TabletRarity::Unique => {
            base.replace('_', " ")
        }
        Some(TabletIdentity::Name { .. }) => {
            q.name.split(" · ").next().unwrap_or(&q.name).to_owned()
        }
        _ => q.name.clone(),
    };
    !name.is_empty()
        && counts.get(&name) == Some(&1)
        && (name == identity || name == english || name == localized)
}

pub fn plan_resources(
    profile: &Profile,
    snapshot: Option<&MarketSnapshot>,
    restore: bool,
    version: &str,
    rules: &AnnotationRules,
) -> Result<PatchPlan> {
    ensure!(profile.valid_context(), "方案包含其他游戏的资源或市场");
    ensure!(rules.valid(), "规则参数不在支持范围内");
    let installation = profile
        .installation
        .as_ref()
        .context("请先选择游戏安装目录")?;
    if !restore && profile.features.iter().any(|f| *f != Feature::MapHints) {
        ensure!(
            profile.market_matches_client(),
            "行情市场与目标客户端服区不一致，请选择匹配市场后再生成价格补丁"
        );
    }
    match installation.storage {
        StorageKind::Bundles => {}
        StorageKind::Ggpk => {}
        StorageKind::Unknown => bail!("未识别到受支持的游戏资源目录"),
    }
    let actual = crate::inspect_for_game(&installation.root, profile.game)?;
    ensure!(
        actual.can_write,
        "此安装尚不支持资源写入：{}",
        actual.reasons.join("；")
    );
    ensure!(actual.id == installation.id, "安装身份发生变化");
    if let Some(s) = snapshot {
        ensure!(
            profile.market.as_ref() == Some(&s.scope),
            "行情市场与方案不一致"
        );
        if !restore {
            ensure!(
                s.quotes.iter().all(|q| q.permits_market(&s.scope)),
                "报价来源市场不符：仅国服传奇允许相同模式的国际参考价"
            );
        }
    }
    let loose = BundleStore::open(&actual.root)?;
    let mut planner = Planner {
        profile,
        snapshot,
        rules,
        loose,
        desired: vec![],
        quotes: BTreeSet::new(),
        warnings: BTreeSet::new(),
        replacements: BTreeMap::new(),
    };
    if !restore {
        planner.names(false)?;
        planner.names(true)?;
        if profile.game == GameId::Poe2 {
            planner.islands()?;
            planner.affixes()?;
        }
    }
    planner
        .desired
        .sort_by(|a, b| (&a.resource, &a.record_id).cmp(&(&b.resource, &b.record_id)));
    let previous = planner
        .loose
        .owned
        .as_ref()
        .map(|o| o.fields.clone())
        .unwrap_or_default();
    let mut changes = vec![];
    for desired in &planner.desired {
        let before = previous
            .iter()
            .find(|o| o.resource == desired.resource && o.record_id == desired.record_id)
            .map(|o| o.after.clone())
            .unwrap_or(desired.before.clone());
        if before != desired.after {
            let mut c = desired.clone();
            c.before = before;
            changes.push(c);
        }
    }
    for old in &previous {
        if !planner
            .desired
            .iter()
            .any(|c| c.resource == old.resource && c.record_id == old.record_id)
        {
            let mut c = old.clone();
            c.before = old.after.clone();
            c.after = old.before.clone();
            c.reason = "清除本应用拥有的标注，恢复原值".into();
            changes.push(c);
        }
    }
    let layers = Feature::for_game(profile.game)
        .iter()
        .copied()
        .map(|feature| LayerReport {
            feature,
            enabled: !restore && profile.features.contains(&feature),
            records: planner
                .desired
                .iter()
                .filter(|c| c.feature == feature)
                .count(),
            changed: changes.iter().filter(|c| c.feature == feature).count(),
            skipped: if restore || !profile.features.contains(&feature) {
                0
            } else {
                snapshot
                    .map(|s| {
                        s.quotes
                            .iter()
                            .filter(|q| {
                                q.feature() == feature && !planner.quotes.contains(&q.item_id)
                            })
                            .count()
                    })
                    .unwrap_or(0)
            },
            detail: if restore || !profile.features.contains(&feature) {
                "恢复本层原始值"
            } else {
                "仅修改可唯一关联且满足格式与质量约束的资源"
            }
            .into(),
        })
        .collect();
    let expires_at = planner
        .desired
        .iter()
        .filter_map(|f| f.price_evidence.as_ref().map(|e| e.expires_at))
        .min();
    let has_prices = !planner.quotes.is_empty();
    let owned_scope = (!restore).then_some(profile.market.clone()).flatten();
    let (mutations, read_set) =
        planner
            .loose
            .materialize(planner.replacements, planner.desired, owned_scope)?;
    let mut plan = PatchPlan {
        game: profile.game,
        id: String::new(),
        expires_at,
        protocol: 3,
        intent: if restore {
            PlanIntent::ClearAnnotations
        } else {
            PlanIntent::Annotate
        },
        installation_id: actual.id,
        installation_root: actual.root,
        installation_fingerprint: actual.fingerprint,
        profile_revision: profile.revision,
        scope: has_prices.then(|| snapshot.unwrap().scope.clone()),
        snapshot_id: has_prices.then(|| snapshot.unwrap().id.clone()),
        quote_ids: planner.quotes.into_iter().collect(),
        layers,
        rule_version: version.into(),
        created_at: Utc::now(),
        changes,
        mutations,
        read_set,
        warnings: planner.warnings.into_iter().collect(),
        blockers: vec![],
    };
    if plan.mutations.is_empty() {
        plan.warnings
            .push("资源与所选方案一致，没有需要写入的变化".into());
    }
    plan.seal()?;
    Ok(plan)
}

fn tablet_slug(identity: &str) -> Option<&'static str> {
    // Stable BaseItemTypes identities paired with the provider's typed base IDs.
    match identity {
        "Metadata/Items/TowerAugment/BreachAugment" => Some("Breach_Tablet"),
        "Metadata/Items/TowerAugment/ExpeditionAugment" => Some("Expedition_Tablet"),
        "Metadata/Items/TowerAugment/DeliriumAugment" => Some("Delirium_Tablet"),
        "Metadata/Items/TowerAugment/RitualAugment" => Some("Ritual_Tablet"),
        "Metadata/Items/TowerAugment/GenericAugment" => Some("Irradiated_Tablet"),
        "Metadata/Items/TowerAugment/MapBossAugment" => Some("Overseer_Tablet"),
        "Metadata/Items/TowerAugment/AbyssAugment" => Some("Abyss_Tablet"),
        "Metadata/Items/TowerAugment/IncursionAugment" => Some("Temple_Tablet"),
        _ => None,
    }
}

/// Container support alone never grants access to the other game's resources.
pub fn writable_game(root: &std::path::Path, game: GameId) -> Result<()> {
    let mut store = BundleStore::open(root)?;
    let poe1 = store.has("data/baseitemtypes.datc64")?;
    let poe2 = store.has("data/balance/baseitemtypes.datc64")?;
    ensure!(poe1 != poe2, "无法唯一识别游戏资源版本");
    ensure!(poe1 == (game == GameId::Poe1), "资源属于另一款游戏");
    let prefix = if poe1 { "data" } else { "data/balance" };
    for table in ["BaseItemTypes", "Words"] {
        for dir in [prefix.to_owned(), format!("{prefix}/simplified chinese")] {
            let path = format!("{dir}/{}.datc64", table.to_lowercase());
            if !store.has(&path)? {
                continue;
            }
            Table::parse_for_game(game, table, store.resource(&path)?)?;
        }
    }
    Ok(())
}
