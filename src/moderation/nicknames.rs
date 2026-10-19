//! Local gift-thanks aliases, resolved before proxy/storage startup.
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

pub type Nicknames = BTreeMap<String, String>;
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NicknameMapping {
    pub file: Option<PathBuf>,
    pub users: Nicknames,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Table {
    version: u32,
    platforms: BTreeMap<String, Nicknames>,
}
pub fn validate(users: &Nicknames) -> Result<()> {
    if users.len() > 10000 {
        bail!("nickname_mapping allows at most 10000 users per platform");
    }
    for (id, name) in users {
        if !super::events::valid_user_id(id) || id.trim() != id || id.trim().is_empty() {
            bail!(
                "nickname_mapping requires a nonempty full user ID without surrounding whitespace, at most 256 characters"
            );
        }
        if name.trim().is_empty() || name.chars().count() > 32 || name.chars().any(char::is_control)
        {
            bail!("nickname_mapping aliases require 1..32 characters without controls");
        }
    }
    Ok(())
}
impl NicknameMapping {
    pub fn resolve(&mut self, base: &Path, platform: &str) -> Result<()> {
        validate(&self.users)?;
        if let Some(path) = &self.file {
            if path.as_os_str().is_empty() {
                bail!("nickname_mapping.file cannot be empty");
            }
            let path = if path.is_relative() {
                base.join(path)
            } else {
                path.clone()
            };
            let mut source = String::new();
            File::open(&path)
                .context("open nickname mapping YAML")?
                .take(1048577)
                .read_to_string(&mut source)
                .context("read nickname mapping YAML")?;
            if source.len() > 1048576 {
                bail!("nickname mapping YAML exceeds 1 MiB");
            }
            let mut table: Table =
                serde_yaml_ng::from_str(&source).context("parse nickname mapping YAML")?;
            if table.version != 1 {
                bail!("unsupported nickname mapping version (expected 1)");
            }
            for (key, users) in &table.platforms {
                if !super::protocol::identifier(key, 64) {
                    bail!("invalid nickname mapping platform key");
                }
                validate(users)?;
            }
            let mut selected = table.platforms.remove(platform).unwrap_or_default();
            selected.extend(std::mem::take(&mut self.users)); // room-specific overrides win
            self.users = selected;
            self.file = Some(path);
        }
        validate(&self.users)?;
        for name in self.users.values_mut() {
            *name = name.trim().into();
        }
        Ok(())
    }
}
