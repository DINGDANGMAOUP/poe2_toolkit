//! Editing is independent from panel visibility and keeps its original revision.
use poe2_core::Profile;

#[derive(Clone)]
pub(crate) struct DraftSession {
    pub original: Profile,
    pub profile: Profile,
}
impl DraftSession {
    pub fn new(profile: &Profile) -> Self {
        Self {
            original: profile.clone(),
            profile: profile.clone(),
        }
    }
    pub fn dirty(&self) -> bool {
        !same_rules(&self.original, &self.profile)
    }
    pub fn current(&self, saved: &Profile) -> bool {
        self.original.game == saved.game
            && self.original.id == saved.id
            && self.original.revision == saved.revision
    }
    pub fn acknowledged(&self, saved: &Profile) -> bool {
        self.original.game == saved.game
            && self.original.id == saved.id
            && saved.revision > self.original.revision
            && same_rules(saved, &self.profile)
    }
}
fn same_rules(a: &Profile, b: &Profile) -> bool {
    a.features == b.features
        && a.annotation == b.annotation
        && a.auto_refresh == b.auto_refresh
        && a.refresh_minutes == b.refresh_minutes
        && a.auto_apply == b.auto_apply
}
