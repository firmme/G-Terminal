use super::{Catalog, Preference, ensure_ids};
use crate::config::{RemoteProfile, SerialProfile, Settings};
use serde_json::json;
use std::cell::{Cell, RefCell};

fn settings() -> Settings {
    let mut settings = Settings {
        profiles: vec![RemoteProfile {
            host: "example.com".into(),
            user: "deploy".into(),
            identity: "C:/private/deploy".into(),
            group: "prod".into(),
            ..Default::default()
        }],
        serial_profiles: vec![SerialProfile {
            port: "COM7".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    ensure_ids(&mut settings);
    settings
}

#[test]
fn portable_catalog_excludes_local_paths_ports_and_workspace() {
    let mut settings = settings();
    settings.profiles[0].jump = Some(Box::new(RemoteProfile {
        host: "jump.example.com".into(),
        identity: "secret-jump-key".into(),
        ..Default::default()
    }));
    let catalog = Catalog::from_settings(&settings);
    let raw = serde_json::to_string(&catalog).unwrap();
    for secret in [
        "C:/private/deploy",
        "secret-jump-key",
        "COM7",
        "workspace",
        "font_size",
    ] {
        assert!(!raw.contains(secret), "leaked {secret}");
    }
    let identity = settings.profiles[0].identity.clone();
    catalog.apply(&mut settings).unwrap();
    assert_eq!(settings.profiles[0].identity, identity);
    assert_eq!(
        settings.profiles[0].jump.as_ref().unwrap().identity,
        "secret-jump-key"
    );
    assert_eq!(settings.serial_profiles[0].port, "COM7");
    let mut other = Settings::default();
    catalog.apply(&mut other).unwrap();
    assert!(other.profiles[0].identity.is_empty());
    assert_eq!(other.serial_profiles[0].port, "auto");
}

#[test]
fn migration_is_stable_across_platforms_and_edits() {
    let first = settings();
    let mut other = first.clone();
    other.profiles[0].sync_id.clear();
    other.profiles[0].identity = "/home/user/.ssh/id_ed25519".into();
    other.serial_profiles[0].sync_id.clear();
    other.serial_profiles[0].port = "/dev/ttyUSB0".into();
    ensure_ids(&mut other);
    assert_eq!(first.profiles[0].sync_id, other.profiles[0].sync_id);
    assert_eq!(
        first.serial_profiles[0].sync_id,
        other.serial_profiles[0].sync_id
    );
    other.profiles[0].name = "renamed".into();
    ensure_ids(&mut other);
    assert_eq!(first.profiles[0].sync_id, other.profiles[0].sync_id);
    other.profiles.push(other.profiles[0].clone());
    ensure_ids(&mut other);
    assert_ne!(other.profiles[0].sync_id, other.profiles[1].sync_id);
}

#[test]
fn concurrent_fields_and_new_connections_merge() {
    let base = Catalog::from_settings(&settings());
    let id = base.ssh.keys().next().unwrap().clone();
    let mut local = base.clone();
    let mut cloud = base.clone();
    local.ssh.get_mut(&id).unwrap()["name"] = json!("new name");
    cloud.ssh.get_mut(&id).unwrap()["port"] = json!(2222);
    let merged = Catalog::merge(&base, &local, &cloud, Preference::Ask).unwrap();
    assert_eq!(merged.ssh[&id]["name"], "new name");
    assert_eq!(merged.ssh[&id]["port"], 2222);
    let mut cloud_settings = settings();
    cloud_settings.profiles[0].host = "new.example.com".into();
    cloud_settings.profiles[0].sync_id.clear();
    ensure_ids(&mut cloud_settings);
    cloud
        .ssh
        .extend(Catalog::from_settings(&cloud_settings).ssh);
    assert_eq!(
        Catalog::merge(&base, &local, &cloud, Preference::Ask)
            .unwrap()
            .ssh
            .len(),
        2
    );
}

#[test]
fn deletes_propagate_but_delete_edit_requires_choice() {
    let base = Catalog::from_settings(&settings());
    let mut deleted = base.clone();
    deleted.ssh.clear();
    assert!(
        Catalog::merge(&base, &base, &deleted, Preference::Ask)
            .unwrap()
            .ssh
            .is_empty()
    );
    let mut edited = base.clone();
    let id = base.ssh.keys().next().unwrap();
    edited.ssh.get_mut(id).unwrap()["name"] = json!("edited offline");
    assert!(Catalog::merge(&base, &edited, &deleted, Preference::Ask).is_err());
    assert_eq!(
        Catalog::merge(&base, &edited, &deleted, Preference::Local).unwrap(),
        edited
    );
    assert_eq!(
        Catalog::merge(&base, &edited, &deleted, Preference::Cloud).unwrap(),
        deleted
    );
}

#[test]
fn choosing_conflicts_does_not_discard_unrelated_changes() {
    let base = Catalog::from_settings(&settings());
    let id = base.ssh.keys().next().unwrap();
    let mut local = base.clone();
    let mut cloud = base.clone();
    local.ssh.get_mut(id).unwrap()["name"] = json!("local");
    cloud.ssh.get_mut(id).unwrap()["name"] = json!("remote");
    cloud.ssh.get_mut(id).unwrap()["port"] = json!(2222);
    assert!(Catalog::merge(&base, &local, &cloud, Preference::Ask).is_err());
    let merged = Catalog::merge(&base, &local, &cloud, Preference::Local).unwrap();
    assert_eq!(merged.ssh[id]["name"], "local");
    assert_eq!(merged.ssh[id]["port"], 2222);
}

#[test]
fn first_link_and_account_switch_union_the_lists() {
    let local = Catalog::from_settings(&settings());
    let mut settings = settings();
    settings.profiles[0].host = "other.example.com".into();
    settings.profiles[0].sync_id.clear();
    ensure_ids(&mut settings);
    let cloud = Catalog::from_settings(&settings);
    let merged = Catalog::merge(&Catalog::default(), &local, &cloud, Preference::Ask).unwrap();
    assert_eq!(merged.ssh.len(), 2);
}

#[test]
fn oauth_callback_rejects_forgery_duplicates_and_denial() {
    use super::auth::callback_code;
    assert_eq!(
        callback_code(
            "GET /?state=expected&code=a%2Bb HTTP/1.1\r\n\r\n",
            "expected"
        )
        .unwrap(),
        "a+b"
    );
    for request in [
        "GET /?state=wrong&code=secret HTTP/1.1",
        "GET /?state=expected&state=wrong&code=secret HTTP/1.1",
        "GET /?state=expected&code=first&code=second HTTP/1.1",
        "GET /?state=expected&error=access_denied HTTP/1.1",
        "POST /?state=expected&code=secret HTTP/1.1",
    ] {
        assert!(
            callback_code(request, "expected").is_err(),
            "accepted {request}"
        );
    }
}

struct RacingStore {
    cloud: RefCell<Catalog>,
    revision: Cell<u32>,
    attempts: Cell<u32>,
    race_once: Cell<bool>,
}

impl super::drive::Store for RacingStore {
    fn read(&self) -> anyhow::Result<super::drive::Snapshot> {
        Ok(super::drive::Snapshot {
            catalog: self.cloud.borrow().clone(),
            revision: Some(("item".into(), self.revision.get().to_string())),
        })
    }
    fn write(
        &self,
        revision: Option<&(String, String)>,
        catalog: &Catalog,
    ) -> anyhow::Result<bool> {
        self.attempts.set(self.attempts.get() + 1);
        if self.race_once.replace(false) {
            self.cloud
                .borrow_mut()
                .groups
                .insert("another-device".into(), "#123456".into());
            self.revision.set(self.revision.get() + 1);
        }
        if revision.unwrap().1 != self.revision.get().to_string() {
            return Ok(false);
        }
        *self.cloud.borrow_mut() = catalog.clone();
        self.revision.set(self.revision.get() + 1);
        Ok(true)
    }
}

#[test]
fn conditional_write_race_fetches_and_merges_again() {
    let base = Catalog::from_settings(&settings());
    let mut local = base.clone();
    local.groups.insert("local".into(), String::new());
    let store = RacingStore {
        cloud: RefCell::new(base.clone()),
        revision: Cell::new(1),
        attempts: Cell::new(0),
        race_once: Cell::new(true),
    };
    let merged = super::drive::merge_and_upload(&store, &base, &local, Preference::Ask).unwrap();
    assert_eq!(store.attempts.get(), 2);
    assert!(merged.groups.contains_key("local"));
    assert!(merged.groups.contains_key("another-device"));
    assert_eq!(*store.cloud.borrow(), merged);
}

#[test]
fn incompatible_or_invalid_cloud_catalog_is_rejected() {
    let mut catalog = Catalog::from_settings(&settings());
    catalog.version = 99;
    assert!(catalog.apply(&mut Settings::default()).is_err());
    catalog.version = 1;
    let id = catalog.ssh.keys().next().unwrap().clone();
    catalog.ssh.get_mut(&id).unwrap()["host"] = json!("-bad");
    assert!(catalog.validate().is_err());
}
