use std::collections::{BTreeMap, BTreeSet};

use crate::{
    resource::ResourceFile,
    steam::{SteamCache, SteamCacheEntry},
    wiki::{PathKind, WikiCache, WikiCacheEntry},
    Error,
};

pub mod placeholder {
    pub const ROOT: &str = "<root>";
    pub const GAME: &str = "<game>";
    pub const BASE: &str = "<base>";
    pub const HOME: &str = "<home>";
    pub const STORE_USER_ID: &str = "<storeUserId>";
    pub const OS_USER_NAME: &str = "<osUserName>";
    pub const WIN_APP_DATA: &str = "<winAppData>";
    pub const WIN_LOCAL_APP_DATA: &str = "<winLocalAppData>";
    pub const WIN_DOCUMENTS: &str = "<winDocuments>";
    pub const WIN_PUBLIC: &str = "<winPublic>";
    pub const WIN_PROGRAM_DATA: &str = "<winProgramData>";
    pub const WIN_DIR: &str = "<winDir>";
    pub const XDG_DATA: &str = "<xdgData>";
    pub const XDG_CONFIG: &str = "<xdgConfig>";
}

fn do_launch_paths_match(from_steam: Option<String>, from_manifest: Option<String>) -> bool {
    match (from_steam, from_manifest) {
        (None, None) => true,
        (Some(from_steam), from_manifest) => normalize_launch_path(&from_steam) == from_manifest,
        _ => false,
    }
}

fn normalize_launch_path(raw: &str) -> Option<String> {
    if raw.contains("://") {
        return Some(raw.to_string());
    }

    let standardized = raw.replace('\\', "/").replace("//", "/");
    let standardized = standardized
        .trim_end_matches('/')
        .trim_start_matches("./")
        .trim_start_matches('/');

    if standardized.is_empty() || standardized == "." {
        None
    } else {
        Some(format!("{}/{}", placeholder::BASE, standardized))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Os {
    Dos,
    Windows,
    Mac,
    Linux,
    #[default]
    #[serde(other)]
    Other,
}

impl From<&str> for Os {
    fn from(value: &str) -> Self {
        match value.to_lowercase().trim() {
            "windows" => Self::Windows,
            "linux" => Self::Linux,
            "mac" | "macos" => Self::Mac,
            "dos" => Self::Dos,
            _ => Self::Other,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Store {
    Ea,
    Epic,
    Gog,
    GogGalaxy,
    Heroic,
    Lutris,
    Microsoft,
    Origin,
    Prime,
    Steam,
    Uplay,
    OtherHome,
    OtherWine,
    #[default]
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Tag {
    Config,
    Save,
    #[default]
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Manifest(pub BTreeMap<String, Game>);

impl ResourceFile for Manifest {
    const FILE_NAME: &'static str = "data/manifest.yaml";
}

impl Manifest {
    pub fn refresh(
        &mut self,
        overrides: &ManifestOverride,
        wiki_cache: &WikiCache,
        steam_cache: &SteamCache,
    ) -> Result<(), Error> {
        self.0.clear();

        for (title, info) in &wiki_cache.0 {
            if overrides.0.get(title).map(|x| x.omit).unwrap_or(false) {
                continue;
            }

            let mut game = Game::default();
            game.integrate_wiki(info, title);
            for rename in &info.renamed_from {
                if rename.to_lowercase() == title.to_lowercase() || self.0.contains_key(rename) {
                    continue;
                }
                self.0.insert(
                    rename.to_string(),
                    Game {
                        alias: Some(title.to_string()),
                        ..Default::default()
                    },
                );
            }
            if let Some(id) = game.steam.id {
                if let Some(info) = steam_cache.0.get(&id) {
                    game.integrate_steam(info);
                }
            }
            if let Some(overridden) = overrides.0.get(title) {
                game.integrate_overrides(overridden);
            }
            if !game.usable() {
                continue;
            }

            self.0.insert(title.to_string(), game);
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Game {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, GameFileEntry>,
    #[serde(skip_serializing_if = "GogMetadata::is_empty")]
    pub gog: GogMetadata,
    #[serde(skip_serializing_if = "IdMetadata::is_empty")]
    pub id: IdMetadata,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub install_dir: BTreeMap<String, GameInstallDirEntry>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub launch: BTreeMap<String, Vec<LaunchEntry>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub registry: BTreeMap<String, GameRegistryEntry>,
    #[serde(skip_serializing_if = "SteamMetadata::is_empty")]
    pub steam: SteamMetadata,
}

impl Game {
    pub fn integrate_wiki(&mut self, cache: &WikiCacheEntry, title: &str) {
        self.steam = SteamMetadata { id: cache.steam };
        self.gog = GogMetadata { id: cache.gog };
        self.id = IdMetadata {
            flatpak: None,
            gog_extra: cache.gog_side.clone(),
            steam_extra: cache.steam_side.clone(),
        };

        let paths = cache.parse_paths(title.to_string());
        for path in paths {
            let tags = path.tags.clone();
            let tags2 = path.tags.clone();

            match path.kind {
                None | Some(PathKind::File) => {
                    let constraint = GameFileConstraint {
                        os: path.os,
                        store: path.store,
                    };
                    let constraint2 = constraint.clone();

                    self.files
                        .entry(path.composite)
                        .and_modify(|x| {
                            x.tags.extend(tags);
                            if !constraint.is_empty() {
                                x.when.insert(constraint);
                            }
                        })
                        .or_insert_with(|| GameFileEntry {
                            tags: tags2.into_iter().collect(),
                            when: (if constraint2.is_empty() {
                                vec![]
                            } else {
                                vec![constraint2]
                            })
                            .into_iter()
                            .collect(),
                        });
                }
                Some(PathKind::Registry) => {
                    let constraint = GameRegistryConstraint { store: path.store };
                    let constraint2 = constraint.clone();

                    self.registry
                        .entry(path.composite)
                        .and_modify(|x| {
                            x.tags.extend(tags);
                            if !constraint.is_empty() {
                                x.when.insert(constraint);
                            }
                        })
                        .or_insert_with(|| GameRegistryEntry {
                            tags: tags2.into_iter().collect(),
                            when: (if constraint2.is_empty() {
                                vec![]
                            } else {
                                vec![constraint2]
                            })
                            .into_iter()
                            .collect(),
                        });
                }
            }
        }
    }

    pub fn integrate_steam(&mut self, cache: &SteamCacheEntry) {
        if let Some(install_dir) = &cache.install_dir {
            self.install_dir.insert(install_dir.to_string(), GameInstallDirEntry {});
        }

        for incoming in &cache.launch {
            if incoming.executable.is_none()
                || incoming.executable.as_ref().map(|x| x.contains("://")).unwrap_or(false)
                || !matches!(incoming.r#type.as_deref(), None | Some("default" | "none"))
                || incoming.config.betakey.is_some()
                || incoming.config.ownsdlc.is_some()
            {
                continue;
            }

            let os = match incoming.config.oslist.as_deref() {
                Some("windows") => Some(Os::Windows),
                Some("macos" | "macosx") => Some(Os::Mac),
                Some("linux") => Some(Os::Linux),
                _ => None,
            };

            let bit = match incoming.config.osarch.as_deref() {
                Some("32") => Some(32),
                Some("64") => Some(64),
                _ => None,
            };

            let constraint = LaunchConstraint {
                bit,
                os,
                store: Some(Store::Steam),
            };

            let mut found_existing = false;
            for (existing_executable, existing_options) in self.launch.iter_mut() {
                for existing in existing_options {
                    if incoming.arguments == existing.arguments
                        && do_launch_paths_match(incoming.executable.clone(), Some(existing_executable.to_string()))
                        && do_launch_paths_match(incoming.workingdir.clone(), existing.working_dir.clone())
                    {
                        found_existing = true;
                        existing.when.insert(constraint.clone());
                    }
                }
            }
            if !found_existing {
                let Some(key) = incoming.executable.as_ref().and_then(|x| normalize_launch_path(x)) else {
                    continue;
                };

                let candidate = LaunchEntry {
                    arguments: incoming.arguments.clone(),
                    when: vec![constraint.clone()].into_iter().collect(),
                    working_dir: incoming.workingdir.as_ref().and_then(|x| normalize_launch_path(x)),
                };
                self.launch
                    .entry(key)
                    .and_modify(|x| x.push(candidate.clone()))
                    .or_insert_with(|| vec![candidate]);
            }
        }
    }

    pub fn integrate_overrides(&mut self, overridden: &OverrideGame) {
        if let Some(id) = overridden.game.steam.id {
            self.steam.id = Some(id);
        }
        if let Some(id) = overridden.game.gog.id {
            self.gog.id = Some(id);
        }
        if let Some(flatpak) = overridden.game.id.flatpak.as_ref() {
            self.id.flatpak = Some(flatpak.clone());
        }
        self.install_dir.extend(overridden.game.install_dir.clone());
    }

    pub fn usable(&self) -> bool {
        !(self.files.is_empty()
            && self.registry.is_empty()
            && self.steam.is_empty()
            && self.gog.is_empty()
            && self.id.is_empty())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GameFileEntry {
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub tags: BTreeSet<Tag>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub when: BTreeSet<GameFileConstraint>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GameInstallDirEntry {}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GameRegistryEntry {
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub tags: BTreeSet<Tag>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub when: BTreeSet<GameRegistryConstraint>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LaunchEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub when: BTreeSet<LaunchConstraint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GameFileConstraint {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<Os>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<Store>,
}

impl GameFileConstraint {
    pub fn is_empty(&self) -> bool {
        self.os.is_none() && self.store.is_none()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GameRegistryConstraint {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<Store>,
}

impl GameRegistryConstraint {
    pub fn is_empty(&self) -> bool {
        self.store.is_none()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LaunchConstraint {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<Os>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<Store>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SteamMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u32>,
}

impl SteamMetadata {
    pub fn is_empty(&self) -> bool {
        self.id.is_none()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GogMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
}

impl GogMetadata {
    pub fn is_empty(&self) -> bool {
        self.id.is_none()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct IdMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flatpak: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub gog_extra: BTreeSet<u64>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub steam_extra: BTreeSet<u32>,
}

impl IdMetadata {
    pub fn is_empty(&self) -> bool {
        self.flatpak.is_none() && self.gog_extra.is_empty() && self.steam_extra.is_empty()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ManifestOverride(pub BTreeMap<String, OverrideGame>);

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OverrideGame {
    pub omit: bool,
    #[serde(flatten)]
    pub game: Game,
}

impl ResourceFile for ManifestOverride {
    const FILE_NAME: &'static str = "data/manifest-override.yaml";
}
