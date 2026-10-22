//! Controllo degli aggiornamenti: abbina gli addon installati ai progetti di CurseForge e
//! sceglie, per ognuno, il file più recente per la versione del gioco dell'installazione.
//!
//! [`evaluate`] è pura: riceve la scansione, la risposta dei fingerprint e i mod, e non usa né
//! rete né disco. [`check_updates`] la collega al client e alla scansione.

use std::collections::HashMap;

use serde::Serialize;

use crate::addons::scan::{self, InstalledAddon};
use crate::installations::Installation;
use crate::sources::curseforge::{
    CurseForgeClient, CurseForgeError, File, FingerprintMatch, FingerprintMatches, Mod,
    RELEASE_TYPE_RELEASE, Transport,
};

/// Tipo di versione del gioco di CurseForge (`gameVersionTypeId`) per il flavor di
/// un'installazione.
///
/// Da confermare con il test `live_version_types` di `sources::curseforge`, che stampa i tipi
/// reali: 517 (Retail) e 67408 (Classic Era) sono gli id usati dai client noti. Tutti gli altri
/// flavor (`classic`, `anniversary`, `classic_beta`, ma anche PTR e beta, che hanno client e
/// file propri) restano senza tipo finché l'id non è confermato: meglio un errore chiaro che
/// un aggiornamento per la versione sbagliata.
pub fn version_type_for(flavor: &str) -> Option<u32> {
    match flavor.trim_matches('_').to_ascii_lowercase().as_str() {
        "retail" => Some(517),
        "classic_era" => Some(67408),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSummary {
    pub id: u32,
    pub name: String,
    pub date: String,
}

impl From<&File> for FileSummary {
    fn from(file: &File) -> Self {
        Self {
            id: file.id,
            name: if file.display_name.is_empty() {
                file.file_name.clone()
            } else {
                file.display_name.clone()
            },
            date: file.file_date.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: u32,
    pub name: String,
    pub url: Option<String>,
    /// Se CurseForge permette di scaricare i file da un'app di terzi. Sconosciuto vale `false`.
    pub allow_mod_distribution: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum UpdateStatus {
    NotMatched,
    UpToDate,
    UpdateAvailable { latest: FileSummary },
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonUpdate {
    pub folder: String,
    /// Progetto CurseForge; più cartelle possono avere lo stesso progetto (per esempio DBM).
    pub project: Option<Project>,
    pub installed_file: Option<FileSummary>,
    #[serde(flatten)]
    pub status: UpdateStatus,
}

/// Abbina e valuta. `version_type` è il `gameVersionTypeId` dell'installazione.
pub fn evaluate(
    addons: &[InstalledAddon],
    matches: &FingerprintMatches,
    mods: &[Mod],
    version_type: u32,
) -> Vec<AddonUpdate> {
    // Ogni fingerprint di cartella → il match del progetto che la contiene.
    let mut by_fingerprint: HashMap<u64, &FingerprintMatch> = HashMap::new();
    for found in &matches.exact_matches {
        for module in &found.file.modules {
            by_fingerprint.insert(module.fingerprint, found);
        }
    }
    let mods_by_id: HashMap<u32, &Mod> = mods.iter().map(|m| (m.id, m)).collect();

    addons
        .iter()
        .map(|addon| {
            let mut update = AddonUpdate {
                folder: addon.folder.clone(),
                project: None,
                installed_file: None,
                status: UpdateStatus::NotMatched,
            };
            let Some(fingerprint) = addon.fingerprint else {
                update.status = UpdateStatus::Error {
                    message: "Cartella non leggibile: impossibile riconoscerla.".to_owned(),
                };
                return update;
            };
            let Some(found) = by_fingerprint.get(&u64::from(fingerprint)) else {
                return update;
            };
            let project = mods_by_id.get(&found.id);
            update.project = Some(Project {
                id: found.id,
                name: project.map(|m| m.name.clone()).unwrap_or_default(),
                url: project.and_then(|m| m.links.website_url.clone()),
                allow_mod_distribution: project
                    .and_then(|m| m.allow_mod_distribution)
                    .unwrap_or(false),
            });
            update.installed_file = Some(FileSummary::from(&found.file));
            update.status = match newest_release(found, version_type) {
                Some(latest) if is_newer(latest, &found.file) => UpdateStatus::UpdateAvailable {
                    latest: FileSummary::from(latest),
                },
                _ => UpdateStatus::UpToDate,
            };
            update
        })
        .collect()
}

fn for_version_type(file: &File, version_type: u32) -> bool {
    file.sortable_game_versions
        .iter()
        .any(|v| v.game_version_type_id == Some(version_type))
}

/// Il file Release più recente per il tipo di versione, tra quelli che CurseForge indica come
/// ultimi per il progetto.
fn newest_release(found: &FingerprintMatch, version_type: u32) -> Option<&File> {
    found
        .latest_files
        .iter()
        .filter(|f| f.release_type == RELEASE_TYPE_RELEASE && f.is_available)
        .filter(|f| for_version_type(f, version_type))
        .max_by(|a, b| a.file_date.cmp(&b.file_date).then(a.id.cmp(&b.id)))
}

/// Le date sono ISO 8601 in UTC, quindi il confronto come testo segue il tempo; a parità di
/// data decide l'id, che CurseForge assegna in ordine crescente.
fn is_newer(candidate: &File, installed: &File) -> bool {
    candidate.id != installed.id
        && (candidate.file_date.as_str(), candidate.id)
            > (installed.file_date.as_str(), installed.id)
}

/// Scansiona l'installazione, chiede a CurseForge e valuta. Bloccante: va chiamata fuori dal
/// thread principale.
pub fn check_updates<T: Transport>(
    installation: &Installation,
    client: &CurseForgeClient<T>,
) -> Result<Vec<AddonUpdate>, String> {
    let version_type = version_type_for(&installation.flavor).ok_or_else(|| {
        format!(
            "Il controllo degli aggiornamenti non supporta ancora la versione del gioco \"{}\".",
            installation.name
        )
    })?;
    let addons = scan::scan_addons(&installation.path, &installation.flavor)?;
    let fingerprints: Vec<u32> = addons.iter().filter_map(|a| a.fingerprint).collect();
    let matches = client
        .match_fingerprints(&fingerprints)
        .map_err(|e: CurseForgeError| e.to_string())?;
    let mut mod_ids: Vec<u32> = matches.exact_matches.iter().map(|m| m.id).collect();
    mod_ids.sort_unstable();
    mod_ids.dedup();
    let mods = client.get_mods(&mod_ids).map_err(|e| e.to_string())?;
    Ok(evaluate(&addons, &matches, &mods, version_type))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::toc::TocInfo;
    use crate::sources::curseforge::{FileModule, ModLinks, SortableGameVersion};

    const RETAIL: u32 = 517;
    const ERA: u32 = 67408;

    fn addon(folder: &str, fingerprint: Option<u32>) -> InstalledAddon {
        InstalledAddon {
            folder: folder.to_owned(),
            toc_file: None,
            toc: TocInfo::default(),
            modified_ms: None,
            fingerprint,
            error: None,
        }
    }

    fn file(id: u32, date: &str, release_type: u8, version_type: u32, modules: &[u64]) -> File {
        File {
            id,
            mod_id: 1,
            display_name: format!("File {id}"),
            file_name: format!("file-{id}.zip"),
            file_date: date.to_owned(),
            release_type,
            game_versions: Vec::new(),
            sortable_game_versions: vec![SortableGameVersion {
                game_version_name: String::new(),
                game_version_type_id: Some(version_type),
            }],
            file_fingerprint: None,
            modules: modules
                .iter()
                .map(|f| FileModule {
                    name: String::new(),
                    fingerprint: *f,
                })
                .collect(),
            is_available: true,
        }
    }

    fn matches(found: Vec<FingerprintMatch>) -> FingerprintMatches {
        FingerprintMatches {
            exact_matches: found,
            ..FingerprintMatches::default()
        }
    }

    fn project(id: u32, allow: Option<bool>) -> Mod {
        Mod {
            id,
            name: format!("Project {id}"),
            slug: String::new(),
            allow_mod_distribution: allow,
            links: ModLinks {
                website_url: Some(format!("https://www.curseforge.com/wow/addons/p{id}")),
            },
            latest_files_indexes: Vec::new(),
        }
    }

    #[test]
    fn unmatched_and_unreadable_folders() {
        let result = evaluate(
            &[addon("Nope", Some(1)), addon("Broken", None)],
            &matches(Vec::new()),
            &[],
            RETAIL,
        );
        assert_eq!(result[0].status, UpdateStatus::NotMatched);
        assert!(result[0].project.is_none());
        assert!(matches!(result[1].status, UpdateStatus::Error { .. }));
    }

    #[test]
    fn up_to_date_when_the_installed_file_is_the_newest() {
        let installed = file(10, "2026-10-01T00:00:00Z", 1, RETAIL, &[100]);
        let found = FingerprintMatch {
            id: 7,
            file: installed.clone(),
            latest_files: vec![installed],
        };
        let result = evaluate(
            &[addon("Foo", Some(100))],
            &matches(vec![found]),
            &[project(7, Some(true))],
            RETAIL,
        );
        assert_eq!(result[0].status, UpdateStatus::UpToDate);
        let project = result[0].project.as_ref().unwrap();
        assert_eq!(project.name, "Project 7");
        assert!(project.allow_mod_distribution);
        assert_eq!(result[0].installed_file.as_ref().unwrap().id, 10);
    }

    #[test]
    fn update_available_for_a_newer_release() {
        let found = FingerprintMatch {
            id: 7,
            file: file(10, "2026-09-01T00:00:00Z", 1, RETAIL, &[100]),
            latest_files: vec![file(12, "2026-10-01T00:00:00Z", 1, RETAIL, &[])],
        };
        let result = evaluate(
            &[addon("Foo", Some(100))],
            &matches(vec![found]),
            &[project(7, None)],
            RETAIL,
        );
        assert_eq!(
            result[0].status,
            UpdateStatus::UpdateAvailable {
                latest: FileSummary {
                    id: 12,
                    name: "File 12".to_owned(),
                    date: "2026-10-01T00:00:00Z".to_owned()
                }
            }
        );
        // allowModDistribution assente vale false.
        assert!(!result[0].project.as_ref().unwrap().allow_mod_distribution);
    }

    #[test]
    fn betas_and_other_game_versions_are_ignored() {
        let found = FingerprintMatch {
            id: 7,
            file: file(10, "2026-09-01T00:00:00Z", 1, RETAIL, &[100]),
            latest_files: vec![
                file(13, "2026-10-02T00:00:00Z", 2, RETAIL, &[]),
                file(14, "2026-10-03T00:00:00Z", 1, ERA, &[]),
            ],
        };
        let result = evaluate(
            &[addon("Foo", Some(100))],
            &matches(vec![found]),
            &[project(7, Some(true))],
            RETAIL,
        );
        assert_eq!(result[0].status, UpdateStatus::UpToDate);
    }

    #[test]
    fn folders_of_the_same_project_share_it() {
        let found = FingerprintMatch {
            id: 3,
            file: file(20, "2026-10-01T00:00:00Z", 1, RETAIL, &[1, 2]),
            latest_files: Vec::new(),
        };
        let result = evaluate(
            &[addon("DBM-Core", Some(1)), addon("DBM-GUI", Some(2))],
            &matches(vec![found]),
            &[project(3, Some(true))],
            RETAIL,
        );
        assert_eq!(result[0].project, result[1].project);
        assert_eq!(result[0].project.as_ref().unwrap().id, 3);
    }

    #[test]
    fn version_types_by_flavor() {
        assert_eq!(version_type_for("retail"), Some(RETAIL));
        assert_eq!(version_type_for("_retail_"), Some(RETAIL));
        assert_eq!(version_type_for("classic_era"), Some(ERA));
        assert_eq!(version_type_for("classic_beta"), None);
        assert_eq!(version_type_for("anniversary"), None);
        assert_eq!(version_type_for("ptr"), None);
        assert_eq!(version_type_for("beta"), None);
    }

    #[test]
    fn the_payload_has_a_status_tag() {
        let update = AddonUpdate {
            folder: "Foo".to_owned(),
            project: None,
            installed_file: None,
            status: UpdateStatus::UpdateAvailable {
                latest: FileSummary {
                    id: 1,
                    name: "x".to_owned(),
                    date: "d".to_owned(),
                },
            },
        };
        let json = serde_json::to_value(&update).unwrap();
        assert_eq!(json["status"], "updateAvailable");
        assert_eq!(json["latest"]["id"], 1);
        assert_eq!(json["installedFile"], serde_json::Value::Null);
        let json = serde_json::to_value(UpdateStatus::NotMatched).unwrap();
        assert_eq!(json["status"], "notMatched");
    }
}
