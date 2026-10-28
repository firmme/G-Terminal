use crate::config::{RemoteProfile, SerialProfile, Settings, portable_identity};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub version: u32,
    pub ssh: BTreeMap<String, Value>,
    pub serial: BTreeMap<String, Value>,
    pub groups: BTreeMap<String, String>,
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            version: 1,
            ssh: BTreeMap::new(),
            serial: BTreeMap::new(),
            groups: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Preference {
    #[default]
    Ask,
    Local,
    Cloud,
}

/// Deterministic migration lets identical old lists on two machines meet once.
/// Duplicated/new profiles receive their own ID before being saved by the UI.
pub fn ensure_ids(settings: &mut Settings) {
    let mut seen = BTreeSet::new();
    for profile in &mut settings.profiles {
        if profile.sync_id.is_empty() {
            let mut portable = profile.clone();
            strip_identity(&mut portable);
            profile.sync_id = legacy_id("ssh", &portable);
        }
        if !seen.insert(profile.sync_id.clone()) {
            profile.sync_id = uuid::Uuid::new_v4().to_string();
            seen.insert(profile.sync_id.clone());
        }
    }
    for profile in &mut settings.serial_profiles {
        if profile.sync_id.is_empty() {
            let mut portable = profile.clone();
            portable.port = "auto".into();
            profile.sync_id = legacy_id("serial", &portable);
        }
        if !seen.insert(profile.sync_id.clone()) {
            profile.sync_id = uuid::Uuid::new_v4().to_string();
            seen.insert(profile.sync_id.clone());
        }
    }
}

fn legacy_id(kind: &str, value: &impl Serialize) -> String {
    let hash = Sha256::digest(serde_json::to_vec(value).expect("profile serialization"));
    format!("{kind}-{hash:x}")
}

fn strip_identity(profile: &mut RemoteProfile) {
    profile.identity.clear();
    profile.synced_identity.clear();
    if let Some(jump) = &mut profile.jump {
        strip_identity(jump);
    }
}

fn restore_identity(profile: &mut RemoteProfile, old: &RemoteProfile) {
    // Device-specific overrides stay local; portable paths follow cloud edits.
    if !old.identity.is_empty() && portable_identity(&old.identity).is_none() {
        profile.synced_identity = profile.identity.clone();
        profile.identity.clone_from(&old.identity);
    }
    if let (Some(jump), Some(old_jump)) = (&mut profile.jump, &old.jump) {
        restore_identity(jump, old_jump);
    }
}

fn portable_identities(profile: &mut RemoteProfile) {
    profile.identity = portable_identity(&profile.identity)
        .or_else(|| {
            if profile.identity.is_empty() {
                None
            } else {
                portable_identity(&profile.synced_identity)
            }
        })
        .unwrap_or_default();
    profile.synced_identity.clear();
    if let Some(jump) = &mut profile.jump {
        portable_identities(jump);
    }
}

fn valid_identities(profile: &RemoteProfile) -> bool {
    profile.synced_identity.is_empty()
        && (profile.identity.is_empty()
            || portable_identity(&profile.identity).as_deref() == Some(profile.identity.as_str())
                && profile.identity.starts_with(crate::config::HOME_TOKEN))
        && profile
            .jump
            .as_ref()
            .is_none_or(|jump| valid_identities(jump))
}

impl Catalog {
    pub fn from_settings(settings: &Settings) -> Self {
        let mut result = Self::default();
        for profile in &settings.profiles {
            let mut profile = profile.clone();
            portable_identities(&mut profile);
            result.ssh.insert(
                profile.sync_id.clone(),
                serde_json::to_value(profile).unwrap(),
            );
        }
        for profile in &settings.serial_profiles {
            let mut profile = profile.clone();
            profile.port = "auto".into();
            result.serial.insert(
                profile.sync_id.clone(),
                serde_json::to_value(profile).unwrap(),
            );
        }
        for name in settings
            .groups
            .iter()
            .chain(settings.profiles.iter().map(|p| &p.group))
            .chain(settings.serial_profiles.iter().map(|p| &p.group))
        {
            if !name.is_empty() {
                result.groups.insert(
                    name.clone(),
                    settings.group_colors.get(name).cloned().unwrap_or_default(),
                );
            }
        }
        result
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            bail!("云端同步格式版本不受支持，请升级 G-Terminal");
        }
        if self.ssh.len() + self.serial.len() + self.groups.len() > 10_000 {
            bail!("云端连接数量超过限制");
        }
        for (id, value) in &self.ssh {
            let profile: RemoteProfile = serde_json::from_value(value.clone())?;
            if id.is_empty()
                || id.len() > 128
                || &profile.sync_id != id
                || !valid_identities(&profile)
            {
                bail!("云端 SSH 连接格式不正确");
            }
            profile.validate()?;
        }
        for (id, value) in &self.serial {
            let profile: SerialProfile = serde_json::from_value(value.clone())?;
            if id.is_empty() || id.len() > 128 || &profile.sync_id != id || profile.port != "auto" {
                bail!("云端串口连接格式不正确");
            }
            profile.validate()?;
        }
        Ok(())
    }

    /// Apply without replacing device-local paths, ports, preferences or sessions.
    pub fn apply(&self, settings: &mut Settings) -> Result<()> {
        self.validate()?;
        let mut ssh = Vec::new();
        // Keep the local sidebar order, append newly downloaded entries.
        let ssh_ids = ordered_ids(
            settings.profiles.iter().map(|p| &p.sync_id),
            self.ssh.keys(),
        );
        for id in ssh_ids {
            let mut profile: RemoteProfile = serde_json::from_value(self.ssh[&id].clone())?;
            if let Some(old) = settings.profiles.iter().find(|p| p.sync_id == id) {
                restore_identity(&mut profile, old);
            }
            ssh.push(profile);
        }
        let mut serial = Vec::new();
        let serial_ids = ordered_ids(
            settings.serial_profiles.iter().map(|p| &p.sync_id),
            self.serial.keys(),
        );
        for id in serial_ids {
            let mut profile: SerialProfile = serde_json::from_value(self.serial[&id].clone())?;
            if let Some(old) = settings.serial_profiles.iter().find(|p| p.sync_id == id) {
                profile.port.clone_from(&old.port);
            }
            serial.push(profile);
        }
        settings.groups = ordered_ids(settings.groups.iter(), self.groups.keys());
        settings.group_colors = self
            .groups
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        settings.profiles = ssh;
        settings.serial_profiles = serial;
        Ok(())
    }

    /// Absence relative to the last common base represents a deletion. With no
    /// base (first link/account switch), all local and remote entries are additions.
    pub fn merge(base: &Self, local: &Self, cloud: &Self, preference: Preference) -> Result<Self> {
        let value = merge_value(
            Some(&serde_json::to_value(base)?),
            Some(&serde_json::to_value(local)?),
            Some(&serde_json::to_value(cloud)?),
            preference,
            "连接列表",
        )?
        .expect("catalog cannot be deleted");
        let result: Self = serde_json::from_value(value)?;
        result.validate()?;
        Ok(result)
    }
}

fn ordered_ids<'a>(
    local: impl Iterator<Item = &'a String>,
    cloud: impl Iterator<Item = &'a String>,
) -> Vec<String> {
    let cloud: BTreeSet<_> = cloud.collect();
    let mut result: Vec<String> = local.filter(|id| cloud.contains(id)).cloned().collect();
    for id in cloud {
        if !result.contains(id) {
            result.push(id.clone());
        }
    }
    result
}

fn merge_value(
    base: Option<&Value>,
    local: Option<&Value>,
    cloud: Option<&Value>,
    preference: Preference,
    path: &str,
) -> Result<Option<Value>> {
    if local == cloud || cloud == base {
        return Ok(local.cloned());
    }
    if local == base {
        return Ok(cloud.cloned());
    }
    if let (Some(Value::Object(local)), Some(Value::Object(cloud))) = (local, cloud) {
        let empty = serde_json::Map::new();
        let base = base.and_then(Value::as_object).unwrap_or(&empty);
        let keys: BTreeSet<_> = base
            .keys()
            .chain(local.keys())
            .chain(cloud.keys())
            .collect();
        let mut out = serde_json::Map::new();
        for key in keys {
            let child = local
                .get(key)
                .or_else(|| cloud.get(key))
                .or_else(|| base.get(key));
            let label = match key.as_str() {
                "ssh" => "SSH 连接",
                "serial" => "串口连接",
                "groups" => "分组",
                "name" => "名称",
                "host" => "主机地址",
                "user" => "用户名",
                "port" => "端口",
                "group" => "所属分组",
                "color" => "颜色",
                "jump" => "跳板机",
                "forwards" => "端口转发",
                _ => child
                    .and_then(|value| value.get("name"))
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                    .or_else(|| {
                        child
                            .and_then(|value| value.get("host"))
                            .and_then(Value::as_str)
                    })
                    .unwrap_or(key),
            };
            if let Some(value) = merge_value(
                base.get(key),
                local.get(key),
                cloud.get(key),
                preference,
                &format!("{path} / {label}"),
            )? {
                out.insert(key.clone(), value);
            }
        }
        return Ok(Some(Value::Object(out)));
    }
    match preference {
        Preference::Local => Ok(local.cloned()),
        Preference::Cloud => Ok(cloud.cloned()),
        Preference::Ask => bail!(
            "同步冲突：{path} 被两端同时修改（或一端删除）。请选择冲突项使用本机还是云端；其余修改会自动合并。"
        ),
    }
}
