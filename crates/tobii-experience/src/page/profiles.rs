//! The page's profile list (docs/experience-app.md section 4.6), without GTK: Tobii's
//! `CalibrationProfileListItemViewModel` items and the `SettingsPageViewModel` rules for
//! switching, marking for deletion and really deleting.

use tobii_control::Request;
use tobii_gaze::profiles::{GUEST, ProfileStore};

/// What the user does on the page, for deciding when marked profiles are really deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageAction {
    /// The page is left: window closed or minimized (Tobii's sidebar hidden).
    NavigateFrom,
    SwitchProfile,
    CreateNewProfile,
    ImproveCalibration,
    SetupDisplay,
}

impl PageAction {
    /// Tobii's `DeleteProfilesMarkedForDeletion` callers: navigating from the page, and before
    /// a switch or Create new profile; not before Improve calibration or Set up display.
    pub fn commits_deletions(self) -> bool {
        matches!(self, PageAction::NavigateFrom | PageAction::SwitchProfile | PageAction::CreateNewProfile)
    }
}

/// One listed profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileItem {
    pub name: String,
    /// Marked for deletion and not yet deleted.
    pub deleted: bool,
    selected: bool,
    guest: bool,
}

impl ProfileItem {
    fn new(name: String) -> ProfileItem {
        let guest = name == GUEST;
        ProfileItem { name, deleted: false, selected: false, guest }
    }

    /// Tobii's `CanDelete`: `!IsSelected && !IsGuest && !IsDeleted`.
    pub fn can_delete(&self) -> bool {
        !self.selected && !self.guest && !self.deleted
    }

    /// Tobii's `CanUndoDelete`: the row shows Undo in place of the trash.
    pub fn can_undo_delete(&self) -> bool {
        self.deleted
    }

    /// Whether the row's profile button is enabled.
    pub fn is_enabled(&self) -> bool {
        !self.deleted
    }
}

/// Every profile except the current one and `Guest`, in the store's order (by name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileList {
    current: Option<String>,
    items: Vec<ProfileItem>,
}

impl ProfileList {
    /// A fresh list: Tobii rebuilds it from scratch, so earlier delete marks are gone.
    pub fn from_store(store: &ProfileStore) -> tobii_gaze::Result<Self> {
        let current = store.current()?;
        let items = store
            .names()?
            .into_iter()
            .filter(|name| name != GUEST && Some(name) != current.as_ref())
            .map(ProfileItem::new)
            .collect();
        Ok(ProfileList { current, items })
    }

    /// The current profile's name, which the list leaves out.
    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    pub fn items(&self) -> &[ProfileItem] {
        &self.items
    }

    /// Whether the store would list other names or another current profile than this list.
    pub fn is_stale(&self, store: &ProfileStore) -> bool {
        match ProfileList::from_store(store) {
            Ok(fresh) => fresh.current != self.current || !fresh.items.iter().map(|i| &i.name).eq(self.names()),
            Err(_) => false,
        }
    }

    fn names(&self) -> impl Iterator<Item = &String> {
        self.items.iter().map(|item| &item.name)
    }

    fn item_mut(&mut self, name: &str) -> Option<&mut ProfileItem> {
        self.items.iter_mut().find(|item| item.name == name)
    }

    /// A click on a listed profile: the switch request when it is another profile than the
    /// current one and not marked deleted. The list then takes it as current, so a second
    /// click on it does nothing.
    pub fn select(&mut self, name: &str) -> Option<Request> {
        if self.current.as_deref() == Some(name) || !self.item_mut(name)?.is_enabled() {
            return None;
        }
        self.current = Some(name.to_string());
        Some(Request::SwitchProfile { name: name.to_string() })
    }

    /// The trash button: marks the profile at once, without asking; false when it cannot be.
    pub fn mark_deleted(&mut self, name: &str) -> bool {
        match self.item_mut(name) {
            Some(item) if item.can_delete() => {
                item.deleted = true;
                true
            }
            _ => false,
        }
    }

    /// The Undo button: clears the mark.
    pub fn undo(&mut self, name: &str) {
        if let Some(item) = self.item_mut(name) {
            item.deleted = false;
        }
    }

    /// Really deletes every marked profile, as Tobii does on navigate-from and before a switch
    /// or a new profile. Each deleted profile leaves the list; the first failure is returned
    /// after the others were tried, and its profile stays marked.
    pub fn commit_deletions(&mut self, store: &ProfileStore) -> tobii_gaze::Result<()> {
        let mut first_error = None;
        let marked: Vec<String> = self.items.iter().filter(|item| item.deleted).map(|item| item.name.clone()).collect();
        for name in marked {
            match store.delete(&name) {
                Ok(()) => self.items.retain(|item| item.name != name),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Commits the deletions when `action` is one Tobii commits them before.
    pub fn before(&mut self, action: PageAction, store: &ProfileStore) -> tobii_gaze::Result<()> {
        if action.commits_deletions() { self.commit_deletions(store) } else { Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A store holding `names` (empty calibration files) with `current` as current profile.
    fn scratch_store(test: &str, names: &[&str], current: Option<&str>) -> (ProfileStore, Scratch) {
        let directory = std::env::temp_dir().join(format!("tobii-experience-profiles-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        for name in names {
            std::fs::write(directory.join(format!("{name}.setpm")), b"").unwrap();
            std::fs::write(directory.join(format!("{name}@DP-1.setpm")), b"").unwrap();
        }
        let store = ProfileStore::new(&directory);
        if let Some(current) = current {
            store.set_current(current).unwrap();
        }
        (store, Scratch(directory))
    }

    fn listed(list: &ProfileList) -> Vec<&str> {
        list.items().iter().map(|item| item.name.as_str()).collect()
    }

    #[test]
    fn list_excludes_current_and_guest() {
        let (store, _scratch) = scratch_store("excludes", &["Garulf", "Guest", "Alice", "Bob"], Some("Garulf"));
        let list = ProfileList::from_store(&store).unwrap();
        assert_eq!(list.current(), Some("Garulf"));
        assert_eq!(listed(&list), ["Alice", "Bob"]);

        let (store, _scratch) = scratch_store("excludes-no-current", &["Guest", "Alice"], None);
        let list = ProfileList::from_store(&store).unwrap();
        assert_eq!(list.current(), None);
        assert_eq!(listed(&list), ["Alice"]);

        let (store, _scratch) = scratch_store("excludes-guest-current", &["Guest", "Alice"], Some("Guest"));
        assert_eq!(listed(&ProfileList::from_store(&store).unwrap()), ["Alice"]);
    }

    #[test]
    fn list_flags_match_tobii() {
        let (store, _scratch) = scratch_store("flags", &["Garulf", "Alice", "Bob"], Some("Garulf"));
        let mut list = ProfileList::from_store(&store).unwrap();
        let alice = |list: &ProfileList| list.items()[0].clone();
        assert!(alice(&list).can_delete() && !alice(&list).can_undo_delete() && alice(&list).is_enabled());

        assert!(list.mark_deleted("Alice"));
        let marked = alice(&list);
        assert!(!marked.can_delete(), "a marked row shows no trash");
        assert!(marked.can_undo_delete(), "a marked row shows Undo");
        assert!(!marked.is_enabled(), "a marked row's profile button is greyed");
        assert!(!list.mark_deleted("Alice"), "already marked");
        assert!(!list.mark_deleted("Garulf"), "the current profile is not listed");
        assert!(!list.mark_deleted("Guest"));
        assert_eq!(list.select("Alice"), None, "a marked profile cannot be switched to");

        list.undo("Alice");
        assert_eq!(alice(&list), ProfileList::from_store(&store).unwrap().items()[0]);

        list.mark_deleted("Bob");
        let rebuilt = ProfileList::from_store(&store).unwrap();
        assert!(rebuilt.items().iter().all(|item| !item.deleted), "a rebuild discards the marks");
        assert!(!list.is_stale(&store), "marks alone do not make the list stale");
    }

    #[test]
    fn deletions_commit_when_tobii_does() {
        for action in [
            PageAction::NavigateFrom,
            PageAction::SwitchProfile,
            PageAction::CreateNewProfile,
            PageAction::ImproveCalibration,
            PageAction::SetupDisplay,
        ] {
            let (store, scratch) =
                scratch_store(&format!("commit-{action:?}"), &["Garulf", "Alice", "Bob"], Some("Garulf"));
            let mut list = ProfileList::from_store(&store).unwrap();
            list.mark_deleted("Alice");
            assert_eq!(store.names().unwrap(), ["Alice", "Bob", "Garulf"], "marking deletes nothing");

            list.before(action, &store).unwrap();
            let commits =
                matches!(action, PageAction::NavigateFrom | PageAction::SwitchProfile | PageAction::CreateNewProfile);
            assert_eq!(action.commits_deletions(), commits, "{action:?}");
            if commits {
                assert_eq!(store.names().unwrap(), ["Bob", "Garulf"], "{action:?}");
                assert!(!scratch.0.join("Alice@DP-1.setpm").exists(), "every calibration goes");
                assert_eq!(listed(&list), ["Bob"]);
            } else {
                assert_eq!(store.names().unwrap(), ["Alice", "Bob", "Garulf"], "{action:?}");
                assert!(list.items()[0].deleted, "the mark stays for later");
            }
        }
    }

    #[test]
    fn select_sends_switch_request() {
        let (store, _scratch) = scratch_store("select", &["Garulf", "Alice"], Some("Garulf"));
        let mut list = ProfileList::from_store(&store).unwrap();
        assert_eq!(list.select("Alice"), Some(Request::SwitchProfile { name: "Alice".into() }));
        assert_eq!(list.current(), Some("Alice"));
        assert_eq!(list.select("Alice"), None, "an equal name does nothing");
        assert_eq!(list.select("Nobody"), None);
        assert!(list.is_stale(&store), "the store has not switched yet");
    }
}
