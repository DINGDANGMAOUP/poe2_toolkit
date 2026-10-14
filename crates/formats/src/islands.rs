use super::*;

// English rumours from the reference project's international restore seed,
// paired with its poe2_island_rumour_patch.py catalogue. Match content rather
// than its fixed RUMOUR_ROWS; WorldAreas' unrelated columns need not be parsed.
const RUMOURS: [(&str, &str); 20] = [
    ("All that glitters...", "Castaway"),
    ("Almost paradise.", "Untainted Paradise"),
    ("Reflective waters...", "The Fractured Lake"),
    ("A good fellow...", "Moment of Zen"),
    ("Crazed Chieftain...", "The Jade Isles"),
    ("Somethin' fishy...", "Barren Atoll"),
    ("End of the circle...", "Sprawling Jungle"),
    ("The last to fall...", "Mournful Cliffside"),
    ("Stardrinker...", "Secluded Temple"),
    ("Origin of the fall...", "Obscure Island"),
    ("Nothin' to drink...", "Stagnant Basin"),
    ("Unknown ruins...", "Exhumed Ruins"),
    ("It's dry at least...", "Sloughed Gully"),
    ("Fallen stars...", "Moor of Fallen Skies"),
    ("Endless cliffs...", "Craggy Peninsula"),
    ("Warm but risky...", "Grazed Prairie"),
    ("Bleak and awful...", "Bleached Shoals"),
    ("Wild roaming free...", "Lush Isle"),
    ("Cold as ice...", "Frigid Bluffs"),
    ("Sulphite!", "Scorched Cay"),
];

impl Planner<'_> {
    pub(super) fn islands(&mut self) -> Result<()> {
        if !self.enabled(Feature::MapHints) {
            return Ok(());
        }
        let path = format!("{LOCAL}/endgamemaps.datc64");
        let english_path = format!("{ENGLISH}/endgamemaps.datc64");
        if !self.loose.has(&path)? || !self.loose.has(&english_path)? {
            self.warnings
                .insert("缺少中英文 EndgameMaps 资源，岛屿提示未修改".into());
            return Ok(());
        }
        let maps = self.table(LOCAL, "EndgameMaps")?;
        let english = self.table(ENGLISH, "EndgameMaps")?;
        ensure!(
            maps.rows == english.rows && maps.layout.row_size == english.layout.row_size,
            "EndgameMaps 中英文表结构不一致，未修改资源"
        );
        let mut rumours = Vec::with_capacity(maps.rows);
        let mut counts = BTreeMap::<String, usize>::new();
        let mut area_counts = BTreeMap::<usize, usize>::new();
        for row in 0..maps.rows {
            let area = english.reference(row, "WorldArea")?;
            ensure!(
                maps.reference(row, "WorldArea")? == area,
                "EndgameMaps 中英文区域引用不一致，未修改资源"
            );
            if let Some(area) = area {
                *area_counts.entry(area).or_default() += 1;
            }
            let rumour = english.text(row, "RumourText")?;
            *counts.entry(rumour.clone()).or_default() += 1;
            rumours.push((area, rumour));
        }
        let mut changes = BTreeMap::new();
        let mut skipped = 0;
        for (row, (area, rumour)) in rumours.iter().enumerate() {
            if rumour.is_empty() {
                continue;
            }
            let Some((_, name)) = RUMOURS.iter().find(|(text, _)| *text == rumour) else {
                skipped += 1;
                continue;
            };
            let identity = format!("island:{name}");
            let Some(hint) = self.rules.islands.iter().find(|hint| hint.id == identity) else {
                continue;
            };
            let before = maps.text(row, "RumourText")?;
            if counts.get(rumour) != Some(&1)
                || !area.is_some_and(|area| area_counts.get(&area) == Some(&1))
                || before.trim().is_empty()
            {
                skipped += 1;
                continue;
            }
            let after = format!("{before} [{}]", hint.text());
            self.record(
                &path,
                &identity,
                Feature::MapHints,
                &before,
                &after,
                "唯一英文传言与中英文区域引用关联；保留原传言",
            );
            changes.insert((row, "RumourText".into()), after);
        }
        if skipped != 0 {
            self.warnings.insert(format!(
                "{skipped} 条岛屿传言尚未收录或无法唯一关联，已保留原文"
            ));
        }
        if changes.is_empty() {
            self.warnings
                .insert("当前 EndgameMaps 没有唯一匹配的岛屿传言".into());
        } else {
            let bytes = maps.rewrite(&changes)?;
            let verify = Table::parse("EndgameMaps", bytes.clone())?;
            for ((row, field), text) in changes {
                ensure!(verify.text(row, &field)? == text, "DAT 传言读回不一致");
            }
            self.replacements.insert(path, bytes);
        }
        Ok(())
    }
}
