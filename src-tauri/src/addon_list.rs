//! La stringa di import/export degli addon di un'installazione (issue #133).
//!
//! Formato: `WOWDON1:` seguito da base64url senza padding di un JSON
//! `{ "v": 1, "flavor": "...", "addons": [{ "source": "curseforge", "key": "1234", "name": "...",
//! "versionType": 517 }] }`. La stringa è un dato che altri incollano, quindi non fidato: porta
//! solo fonte, chiave, nome e tipo di versione del gioco, mai percorsi o URL, e si legge con
//! limiti di dimensione e di voci. Una voce non valida si salta e si riporta; tutto il resto
//! vale.

use std::collections::HashSet;

use serde::Serialize;
use serde_json::Value;

use crate::addons::scan::ListedAddon;
use crate::store::{SourceKind, SourceRecords};
use crate::updates::{Refresh, VersionOverrides, is_known_version_type};

/// Il prefisso della versione 1 del formato.
pub const PREFIX: &str = "WOWDON1:";
/// La versione del JSON dentro la stringa.
const FORMAT_VERSION: u64 = 1;
/// La stringa più lunga accettata, in byte, dopo aver tolto gli spazi intorno.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
/// Il numero massimo di voci di una stringa.
pub const MAX_ENTRIES: usize = 1000;
const MAX_KEY_CHARS: usize = 64;
const MAX_NAME_CHARS: usize = 200;
const MAX_FLAVOR_CHARS: usize = 64;

/// Un progetto della stringa.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListEntry {
    pub source: SourceKind,
    pub key: String,
    pub name: String,
    /// La versione del gioco forzata per il progetto, se c'è (`gameVersionTypeId`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_type: Option<u32>,
}

/// Gli addon di un'installazione, come viaggiano nella stringa.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AddonList {
    pub v: u64,
    /// Il flavor dell'installazione da cui viene (`retail`, `classic_era`, ...).
    pub flavor: String,
    pub addons: Vec<ListEntry>,
}

/// Una voce della stringa che non si è potuta leggere: la posizione (da 1) e il motivo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvalidEntry {
    pub position: usize,
    pub reason: String,
}

/// Il risultato di [`decode`]: le voci valide e quelle saltate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    pub list: AddonList,
    pub invalid: Vec<InvalidEntry>,
}

/// La stringa di `list`.
pub fn encode(list: &AddonList) -> String {
    // Un `AddonList` ha solo stringhe, numeri e fonti note: la serializzazione non può fallire.
    let json = serde_json::to_vec(list).unwrap_or_default();
    format!("{PREFIX}{}", base64url_encode(&json))
}

/// Legge una stringa incollata. Rifiuta tutta la stringa se il prefisso, la versione, la base64
/// o il JSON non vanno, o se supera i limiti; una voce non valida invece si salta e finisce in
/// `invalid`. Una voce ripetuta (stessa fonte e chiave) vale una volta sola. Un `versionType`
/// che l'app non conosce si ignora: il progetto si installa con la versione dell'installazione.
pub fn decode(text: &str) -> Result<Decoded, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Paste an addon list first.".to_owned());
    }
    if text.len() > MAX_TEXT_BYTES {
        return Err("The addon list is too long.".to_owned());
    }
    let Some(payload) = text.strip_prefix(PREFIX) else {
        if is_newer_prefix(text) {
            return Err(
                "This addon list comes from a newer version of WoW-don: update the app.".to_owned(),
            );
        }
        return Err("This is not a WoW-don addon list.".to_owned());
    };
    let bytes = base64url_decode(payload.trim())
        .ok_or_else(|| "The addon list is damaged (invalid characters).".to_owned())?;
    let root: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "The addon list is damaged (unreadable content).".to_owned())?;
    let object = root
        .as_object()
        .ok_or_else(|| "The addon list is damaged (unexpected content).".to_owned())?;
    match object.get("v").and_then(Value::as_u64) {
        Some(FORMAT_VERSION) => {}
        Some(_) => {
            return Err(
                "This addon list comes from a newer version of WoW-don: update the app.".to_owned(),
            );
        }
        None => return Err("The addon list is damaged (no version).".to_owned()),
    }
    let flavor = object
        .get("flavor")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if flavor.chars().count() > MAX_FLAVOR_CHARS {
        return Err("The addon list is damaged (game version too long).".to_owned());
    }
    let entries = object
        .get("addons")
        .and_then(Value::as_array)
        .ok_or_else(|| "The addon list is damaged (no addons).".to_owned())?;
    if entries.len() > MAX_ENTRIES {
        return Err(format!(
            "The addon list has more than {MAX_ENTRIES} addons."
        ));
    }

    let mut addons = Vec::new();
    let mut invalid = Vec::new();
    let mut seen = HashSet::new();
    for (index, entry) in entries.iter().enumerate() {
        match read_entry(entry) {
            Ok(entry) => {
                if seen.insert((entry.source, entry.key.clone())) {
                    addons.push(entry);
                }
            }
            Err(reason) => invalid.push(InvalidEntry {
                position: index + 1,
                reason,
            }),
        }
    }
    Ok(Decoded {
        list: AddonList {
            v: FORMAT_VERSION,
            flavor: flavor.to_owned(),
            addons,
        },
        invalid,
    })
}

/// `WOWDON<n>:` con `n` diverso da 1: una stringa di un formato futuro.
fn is_newer_prefix(text: &str) -> bool {
    text.strip_prefix("WOWDON")
        .and_then(|rest| rest.split_once(':'))
        .is_some_and(|(version, _)| {
            !version.is_empty() && version.bytes().all(|b| b.is_ascii_digit())
        })
}

fn read_entry(entry: &Value) -> Result<ListEntry, String> {
    let object = entry
        .as_object()
        .ok_or_else(|| "not an addon entry".to_owned())?;
    let source_name = object
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| "no source".to_owned())?;
    let source: SourceKind = serde_json::from_value(Value::String(source_name.to_owned()))
        .map_err(|_| "unknown source".to_owned())?;
    let key = object
        .get("key")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if key.is_empty() {
        return Err("no project id".to_owned());
    }
    if key.chars().count() > MAX_KEY_CHARS || key.chars().any(char::is_control) {
        return Err("invalid project id".to_owned());
    }
    if source == SourceKind::Curseforge && key.parse::<u32>().is_err() {
        return Err("invalid CurseForge project id".to_owned());
    }
    let name = match object.get("name") {
        None | Some(Value::Null) => "",
        Some(Value::String(name)) => name.trim(),
        Some(_) => return Err("invalid name".to_owned()),
    };
    if name.chars().count() > MAX_NAME_CHARS {
        return Err("name too long".to_owned());
    }
    let version_type = object
        .get("versionType")
        .and_then(Value::as_u64)
        .and_then(|id| u32::try_from(id).ok())
        .filter(|&id| is_known_version_type(id));
    Ok(ListEntry {
        source,
        key: key.to_owned(),
        // I caratteri di controllo non servono in un nome e confonderebbero l'interfaccia.
        name: name.chars().filter(|c| !c.is_control()).collect(),
        version_type,
    })
}

/// Un addon esportato, per il riepilogo nel frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Export {
    pub text: String,
    pub exported: Vec<ListEntry>,
    /// Le cartelle che nessun progetto riconosce (per esempio addon installati da zip): non si
    /// possono riscaricare, quindi non entrano nella stringa.
    pub skipped: Vec<String>,
    /// Perché il controllo non è riuscito, se è fallito: senza controllo un progetto si riconosce
    /// solo dai record dell'app, quindi `skipped` può contenere cartelle che hanno un progetto.
    pub check_error: Option<String>,
}

/// La lista di un'installazione dai risultati di una scansione (`refresh`) e dai record
/// dell'app (`records`). Un progetto riconosciuto dal controllo entra con il suo nome e la sua
/// versione forzata; un progetto registrato che il controllo non ha riconosciuto (per esempio
/// CurseForge senza chiave) entra se almeno una sua cartella è ancora sul disco. Le cartelle
/// che nessun progetto copre restano in `skipped`, per nome. Una voce esce con gli stessi limiti
/// che [`decode`] controlla: un nome troppo lungo si accorcia, una chiave non valida resta fuori
/// e le sue cartelle finiscono in `skipped`, così ogni stringa esportata si rilegge per intero.
pub fn export_list(
    flavor: &str,
    refresh: &Refresh,
    records: &SourceRecords,
    overrides: &VersionOverrides,
) -> Export {
    let mut entries: Vec<ListEntry> = Vec::new();
    let mut seen: HashSet<(SourceKind, String)> = HashSet::new();
    let mut covered: HashSet<String> = HashSet::new();

    for update in &refresh.updates {
        let Some(project) = &update.project else {
            continue;
        };
        if !exportable_key(project.source, &project.key) {
            continue;
        }
        covered.insert(update.folder.to_lowercase());
        covered.extend(update.project_folders.iter().map(|f| f.to_lowercase()));
        if seen.insert((project.source, project.key.clone())) {
            entries.push(ListEntry {
                source: project.source,
                key: project.key.clone(),
                name: exportable_name(&project.name),
                version_type: update.version_override,
            });
        }
    }

    for (source, projects) in records {
        for (key, record) in projects {
            if seen.contains(&(*source, key.clone())) {
                continue;
            }
            let present: Vec<&ListedAddon> = record
                .folders
                .iter()
                .filter_map(|folder| folder_named(&refresh.addons, folder))
                .collect();
            let Some(first) = present.first() else {
                continue;
            };
            if !exportable_key(*source, key) {
                continue;
            }
            seen.insert((*source, key.clone()));
            covered.extend(present.iter().map(|a| a.addon.folder.to_lowercase()));
            entries.push(ListEntry {
                source: *source,
                key: key.clone(),
                name: exportable_name(
                    first
                        .addon
                        .toc
                        .title
                        .as_deref()
                        .filter(|title| !title.is_empty())
                        .unwrap_or(&first.addon.folder),
                ),
                // Il controllo non ha riconosciuto il progetto, quindi la sua versione forzata
                // si legge dalle impostazioni (solo CurseForge ne ha).
                version_type: (*source == SourceKind::Curseforge)
                    .then(|| key.parse::<u32>().ok())
                    .flatten()
                    .and_then(|mod_id| overrides.get(&mod_id).copied()),
            });
        }
    }

    let skipped = refresh
        .addons
        .iter()
        .map(|listed| &listed.addon.folder)
        .filter(|folder| !covered.contains(&folder.to_lowercase()))
        .cloned()
        .collect();
    let list = AddonList {
        v: FORMAT_VERSION,
        flavor: flavor.to_owned(),
        addons: entries,
    };
    Export {
        text: encode(&list),
        exported: list.addons,
        skipped,
        check_error: refresh.check_error.clone(),
    }
}

/// Una chiave che [`decode`] accetta per la fonte.
fn exportable_key(source: SourceKind, key: &str) -> bool {
    !key.trim().is_empty()
        && key == key.trim()
        && key.chars().count() <= MAX_KEY_CHARS
        && !key.chars().any(char::is_control)
        && (source != SourceKind::Curseforge || key.parse::<u32>().is_ok())
}

/// Il nome come [`decode`] lo rilegge: senza caratteri di controllo né spazi ai lati, entro
/// [`MAX_NAME_CHARS`].
fn exportable_name(name: &str) -> String {
    let clean: String = name.chars().filter(|c| !c.is_control()).collect();
    clean
        .trim()
        .chars()
        .take(MAX_NAME_CHARS)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

fn folder_named<'a>(addons: &'a [ListedAddon], folder: &str) -> Option<&'a ListedAddon> {
    addons
        .iter()
        .find(|listed| listed.addon.folder.eq_ignore_ascii_case(folder))
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Base64url (RFC 4648, sezione 5) senza padding.
fn base64url_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let symbols = chunk.len() + 1;
        for i in 0..symbols {
            out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 0x3f) as usize]));
        }
    }
    out
}

/// L'inverso di [`base64url_encode`]. Accetta il padding `=` finale (alcuni strumenti lo
/// aggiungono) e niente altro: un carattere fuori dall'alfabeto o una lunghezza impossibile
/// danno `None`.
fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim_end_matches('=');
    if text.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    // I bit avanzati devono essere zero, come li scrive un codificatore corretto.
    if buffer != 0 {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::scan::InstalledAddon;
    use crate::addons::toc::TocInfo;
    use crate::store::{ProjectRecords, SourceRecord};
    use crate::updates::{AddonUpdate, Project, UpdateStatus};

    fn entry(source: SourceKind, key: &str, name: &str, version_type: Option<u32>) -> ListEntry {
        ListEntry {
            source,
            key: key.to_owned(),
            name: name.to_owned(),
            version_type,
        }
    }

    fn encode_json(json: &str) -> String {
        format!("{PREFIX}{}", base64url_encode(json.as_bytes()))
    }

    #[test]
    fn base64url_matches_the_rfc_vectors() {
        // RFC 4648, sezione 10, senza padding.
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg"),
            ("fo", "Zm8"),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg"),
            ("fooba", "Zm9vYmE"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64url_encode(plain.as_bytes()), encoded);
            assert_eq!(base64url_decode(encoded).unwrap(), plain.as_bytes());
        }
        // L'alfabeto url: `-` e `_` al posto di `+` e `/`.
        assert_eq!(base64url_encode(&[0xfb, 0xff]), "-_8");
        assert_eq!(base64url_decode("-_8").unwrap(), [0xfb, 0xff]);
        assert_eq!(base64url_decode("Zm8=").unwrap(), b"fo");
        assert_eq!(base64url_decode("Zm+v"), None);
        assert_eq!(base64url_decode("Z"), None);
        // Bit avanzati non nulli: non è una codifica corretta.
        assert_eq!(base64url_decode("Zh"), None);
    }

    #[test]
    fn base64url_round_trips_every_byte() {
        let data: Vec<u8> = (0..=255u8).chain(0..=255u8).collect();
        for len in 0..data.len() {
            let encoded = base64url_encode(&data[..len]);
            assert_eq!(
                base64url_decode(&encoded).unwrap(),
                &data[..len],
                "len {len}"
            );
        }
    }

    #[test]
    fn a_list_round_trips() {
        let list = AddonList {
            v: 1,
            flavor: "retail".to_owned(),
            addons: vec![
                entry(
                    SourceKind::Curseforge,
                    "61284",
                    "Details!",
                    Some(crate::updates::RETAIL),
                ),
                entry(SourceKind::Tukui, "-2", "ElvUI", None),
            ],
        };
        let text = encode(&list);
        assert!(text.starts_with(PREFIX));
        let decoded = decode(&format!("  \n{text}\n ")).unwrap();
        assert_eq!(decoded.list, list);
        assert!(decoded.invalid.is_empty());
    }

    #[test]
    fn hostile_or_broken_strings_are_refused() {
        for (text, message) in [
            ("", "Paste an addon list first."),
            ("hello", "This is not a WoW-don addon list."),
            ("WOWDON2:abc", "newer version"),
            ("WOWDON1:not base64!", "invalid characters"),
            (&encode_json("[1,2]"), "unexpected content"),
            (&encode_json("{\"v\":1"), "unreadable content"),
            (&encode_json("{\"v\":2,\"addons\":[]}"), "newer version"),
            (&encode_json("{\"addons\":[]}"), "no version"),
            (&encode_json("{\"v\":1}"), "no addons"),
        ] {
            let error = decode(text).unwrap_err();
            assert!(error.contains(message), "{text:?}: {error}");
        }
        let long = format!("{PREFIX}{}", "A".repeat(MAX_TEXT_BYTES));
        assert_eq!(decode(&long).unwrap_err(), "The addon list is too long.");
        let many = format!(
            "{{\"v\":1,\"addons\":[{}]}}",
            vec!["{}"; MAX_ENTRIES + 1].join(",")
        );
        assert!(
            decode(&encode_json(&many))
                .unwrap_err()
                .contains("more than")
        );
    }

    #[test]
    fn invalid_entries_are_skipped_and_reported() {
        let long_key = "k".repeat(65);
        let long_name = "n".repeat(201);
        let json = format!(
            r#"{{"v":1,"flavor":"classic","addons":[
                {{"source":"curseforge","key":"61284","name":" Details! ","versionType":517}},
                {{"source":"overwolf","key":"1"}},
                {{"source":"curseforge","key":""}},
                {{"source":"curseforge","key":"abc"}},
                {{"source":"github","key":"{long_key}"}},
                {{"source":"tukui","key":"-2","name":"{long_name}"}},
                "nope",
                {{"source":"tukui","key":"-1","name":"Tukui","versionType":12345}},
                {{"source":"curseforge","key":"61284","name":"again"}}
            ]}}"#
        );
        let decoded = decode(&encode_json(&json)).unwrap();
        assert_eq!(decoded.list.flavor, "classic");
        assert_eq!(
            decoded.list.addons,
            [
                entry(SourceKind::Curseforge, "61284", "Details!", Some(517)),
                // Un tipo di versione sconosciuto si ignora, la voce resta.
                entry(SourceKind::Tukui, "-1", "Tukui", None),
            ]
        );
        let positions: Vec<usize> = decoded.invalid.iter().map(|i| i.position).collect();
        assert_eq!(positions, [2, 3, 4, 5, 6, 7]);
        assert_eq!(decoded.invalid[0].reason, "unknown source");
    }

    fn listed(folder: &str, title: Option<&str>) -> ListedAddon {
        ListedAddon {
            addon: InstalledAddon {
                folder: folder.to_owned(),
                toc_file: None,
                toc: TocInfo {
                    title: title.map(str::to_owned),
                    ..TocInfo::default()
                },
                modified_ms: None,
                fingerprint: None,
                error: None,
            },
            installed_project: None,
        }
    }

    fn matched(
        folder: &str,
        source: SourceKind,
        key: &str,
        name: &str,
        also: &[&str],
    ) -> AddonUpdate {
        AddonUpdate {
            folder: folder.to_owned(),
            source: Some(source),
            project: Some(Project {
                id: key.parse().unwrap_or(0),
                source,
                key: key.to_owned(),
                name: name.to_owned(),
                url: None,
                allow_mod_distribution: true,
            }),
            installed_file: None,
            status: UpdateStatus::UpToDate,
            version_override: None,
            project_folders: also.iter().map(|f| (*f).to_owned()).collect(),
            installed_modules: Vec::new(),
            reinstall_file: None,
        }
    }

    fn unmatched(folder: &str) -> AddonUpdate {
        AddonUpdate {
            folder: folder.to_owned(),
            source: None,
            project: None,
            installed_file: None,
            status: UpdateStatus::NotMatched,
            version_override: None,
            project_folders: Vec::new(),
            installed_modules: Vec::new(),
            reinstall_file: None,
        }
    }

    #[test]
    fn the_export_has_one_entry_per_project_and_names_the_rest() {
        let mut dbm = matched(
            "DBM-Core",
            SourceKind::Curseforge,
            "3358",
            "Deadly Boss Mods",
            &["DBM-Core", "DBM-StatusBarTimers"],
        );
        dbm.version_override = Some(crate::updates::CLASSIC_ERA);
        let refresh = Refresh {
            addons: vec![
                listed("DBM-Core", None),
                listed("DBM-StatusBarTimers", None),
                listed("ElvUI", None),
                listed("MyZipAddon", None),
                listed("Recorded", Some("Recorded Addon")),
            ],
            updates: vec![
                dbm,
                // Riconosciuta solo come parte del file di DBM.
                unmatched("DBM-StatusBarTimers"),
                matched("ElvUI", SourceKind::Tukui, "-2", "ElvUI", &[]),
                unmatched("MyZipAddon"),
                unmatched("Recorded"),
            ],
            check_error: None,
        };
        // Un progetto registrato che il controllo non ha riconosciuto (senza chiave), uno già
        // esportato e uno senza cartelle sul disco.
        let records = SourceRecords::from([(
            SourceKind::Curseforge,
            ProjectRecords::from([
                (
                    "777".to_owned(),
                    SourceRecord {
                        file: "1".to_owned(),
                        folders: vec!["recorded".to_owned()],
                    },
                ),
                (
                    "3358".to_owned(),
                    SourceRecord {
                        file: "1".to_owned(),
                        folders: vec!["DBM-Core".to_owned()],
                    },
                ),
                (
                    "888".to_owned(),
                    SourceRecord {
                        file: "1".to_owned(),
                        folders: vec!["Gone".to_owned()],
                    },
                ),
            ]),
        )]);
        let overrides = VersionOverrides::from([(777, crate::updates::RETAIL)]);
        let export = export_list("retail", &refresh, &records, &overrides);
        assert_eq!(
            export.exported,
            [
                entry(
                    SourceKind::Curseforge,
                    "3358",
                    "Deadly Boss Mods",
                    Some(crate::updates::CLASSIC_ERA)
                ),
                entry(SourceKind::Tukui, "-2", "ElvUI", None),
                entry(
                    SourceKind::Curseforge,
                    "777",
                    "Recorded Addon",
                    Some(crate::updates::RETAIL)
                ),
            ]
        );
        assert_eq!(export.skipped, ["MyZipAddon"]);
        assert_eq!(export.check_error, None);
        let decoded = decode(&export.text).unwrap();
        assert_eq!(decoded.list.addons, export.exported);
        assert_eq!(decoded.list.flavor, "retail");
    }

    #[test]
    fn every_exported_entry_reads_back() {
        let long_name = format!("  {}\u{7}  ", "N".repeat(250));
        let long_key = "o/".repeat(40);
        let refresh = Refresh {
            addons: vec![listed("Long", None), listed("BadKey", None)],
            updates: vec![
                matched("Long", SourceKind::Tukui, "-3", &long_name, &[]),
                matched("BadKey", SourceKind::Github, &long_key, "Repo", &[]),
            ],
            check_error: Some("network down".to_owned()),
        };
        let export = export_list(
            "retail",
            &refresh,
            &SourceRecords::new(),
            &VersionOverrides::new(),
        );
        assert_eq!(export.exported.len(), 1);
        assert_eq!(export.exported[0].name.chars().count(), MAX_NAME_CHARS);
        assert_eq!(export.skipped, ["BadKey"]);
        assert_eq!(export.check_error.as_deref(), Some("network down"));
        let decoded = decode(&export.text).unwrap();
        assert!(decoded.invalid.is_empty());
        assert_eq!(decoded.list.addons, export.exported);
    }
}
