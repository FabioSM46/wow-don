//! Il mirror GitHub di un progetto CurseForge che non si può scaricare (D34, issue #162).
//!
//! Quando l'autore vieta il download da app di terzi (`allowModDistribution` diverso da
//! `true`, D13), lo stesso addon può essere pubblicato anche nelle release di GitHub. [`resolve`]
//! dice se lo è, e se la release è la stessa del file CurseForge che l'app installerebbe:
//!
//! - il repository si ricava **solo** dal `sourceUrl` del progetto, quando punta a
//!   `github.com/<owner>/<repo>` (con il parsing dei link di `install_link::parse_link`); un
//!   altro host o nessun `sourceUrl` non fa nessuna richiesta. Mai ricerca per nome;
//! - la release si legge con [`GithubSource`]: stessa cache, stesso blocco dopo il limite di
//!   richieste, stessa scelta dello zip dal `release.json` per il flavor del gioco;
//! - la regola della stessa versione è [`same_version`], una funzione pura.
//!
//! Dati verificati il 2026-10-26: Bartender4 (`4.18.3.1`) e Deadly Boss Mods (`12.1.17`) hanno
//! release con `release.json`; Details ha l'ultima release del 2021; Auctionator e Leatrix Plus
//! non hanno release (404).
//!
//! Gli aggiornamenti e la reinstallazione (issue #163) usano il mirror attraverso
//! [`MirrorSource`]: scaricano lo zip di GitHub con [`install`] e scrivono il record
//! **CurseForge** del file che combacia, così il progetto resta abbinato a CurseForge.

use std::path::Path;

use serde::Serialize;

use super::curseforge::{File, Mod};
use super::github::{Asset, ChoiceError, GithubSource, ReleaseError};
use super::http::{DownloadProgress, PublicTransport};
use crate::install_link;
use crate::progress::Reporter;
use crate::sources::SourceKind;
use crate::updates;

/// Quello che la fonte CurseForge chiede a GitHub per un progetto bloccato: l'esito del mirror e
/// il download del suo zip. Un trait a oggetti, così `CurseForgeSource` non ha un secondo
/// parametro di tipo per il trasporto di GitHub.
pub(crate) trait MirrorSource {
    /// [`resolve`] con la fonte GitHub.
    fn resolve(&self, project: &Mod, file: &File, flavor: &str) -> Outcome;

    /// Lo zip `asset`, con le regole di download di `github.rs` (domini, dimensione).
    fn download(
        &self,
        asset: &Asset,
        progress: &mut DownloadProgress<'_>,
    ) -> Result<Vec<u8>, String>;
}

impl<T: PublicTransport> MirrorSource for GithubSource<T> {
    fn resolve(&self, project: &Mod, file: &File, flavor: &str) -> Outcome {
        resolve(self, project, file, flavor)
    }

    fn download(
        &self,
        asset: &Asset,
        progress: &mut DownloadProgress<'_>,
    ) -> Result<Vec<u8>, String> {
        GithubSource::download(self, asset, progress)
    }
}

/// Il mirror di un progetto, come lo vede il frontend: `{ repo, tag }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GithubMirror {
    /// `owner/repo`.
    pub repo: String,
    pub tag: String,
}

/// I campi del mirror in `AddonUpdate` (`githubMirror`, `githubMirrorReason`): entrambi `null`
/// quando il mirror non è stato calcolato (progetto non bloccato, aggiornato o di un'altra
/// fonte).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MirrorStatus {
    pub github_mirror: Option<GithubMirror>,
    /// Perché il mirror non si può usare (D27, in inglese).
    pub github_mirror_reason: Option<String>,
    /// Il progetto e il file da cui calcolare il mirror alla fine del controllo
    /// (`updates::resolve_mirrors`); vuoto dopo. Resta nel backend.
    #[serde(skip)]
    pub pending: Option<Box<Pending>>,
}

/// Il progetto CurseForge bloccato e il file da confrontare con la release di GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub project: Mod,
    pub file: File,
}

impl MirrorStatus {
    pub(crate) fn of(outcome: &Outcome) -> Self {
        match outcome {
            Outcome::Mirror { repo, tag, .. } => Self {
                github_mirror: Some(GithubMirror {
                    repo: repo.clone(),
                    tag: tag.clone(),
                }),
                github_mirror_reason: None,
                pending: None,
            },
            Outcome::Unavailable { reason } => Self {
                github_mirror: None,
                github_mirror_reason: Some(reason.message()),
                pending: None,
            },
        }
    }
}

/// Scarica lo zip `asset` del mirror e lo installa al posto di `old_modules`, con l'estrazione
/// sicura degli aggiornamenti (`updates::install_bytes`). Restituisce le cartelle installate:
/// il record, CurseForge, lo scrive chi chiama (gli aggiornamenti qui, l'installazione dalla
/// Search con la issue #164). `check` può rifiutare le cartelle dello zip prima che tocchino il
/// disco.
pub(crate) fn install(
    github: &dyn MirrorSource,
    asset: &Asset,
    addons_dir: &Path,
    old_modules: &[String],
    check: &dyn Fn(&[String]) -> Result<(), String>,
    reporter: &Reporter<'_>,
) -> Result<Vec<String>, String> {
    reporter.download(0, None);
    let bytes = github.download(asset, &mut |received, total| {
        reporter.download(received, total)
    })?;
    reporter.downloaded(bytes.len() as u64);
    updates::install_bytes(&bytes, addons_dir, old_modules, check, reporter)
}

/// Il messaggio dell'esito di un'installazione dal mirror.
pub(crate) fn installed_message(repo: &str, tag: &str) -> String {
    format!("Installed from GitHub ({repo} {tag}): CurseForge blocks third-party downloads.")
}

/// L'esito della ricerca del mirror.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Lo zip di GitHub da scaricare al posto del file CurseForge.
    Mirror {
        /// `owner/repo`.
        repo: String,
        tag: String,
        asset: Asset,
    },
    Unavailable {
        reason: Reason,
    },
}

/// Perché un progetto non ha un mirror utilizzabile. [`Reason::message`] è il testo in inglese
/// (D27) da mostrare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reason {
    /// CurseForge permette il download: il mirror non serve e GitHub non si interroga.
    NotBlocked,
    /// Il progetto non ha un `sourceUrl` verso un repository GitHub.
    NoGithubSource,
    /// Il repository non esiste o non ha release pubblicate.
    NoRelease(String),
    /// La release non ha uno zip per questa versione del gioco.
    NoZip(String),
    /// La release di GitHub non è il file di CurseForge.
    DifferentVersion { github: String, curseforge: String },
    /// Il limite di richieste di GitHub, con l'ora di sblocco.
    RateLimited(String),
    /// Rete o risposta illeggibile, della release o del suo `release.json`.
    Failed(String),
}

impl Reason {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::NotBlocked => {
                "CurseForge allows downloads of this addon: no GitHub mirror is needed.".to_owned()
            }
            Self::NoGithubSource => {
                "The CurseForge project does not link a GitHub repository.".to_owned()
            }
            Self::DifferentVersion { github, curseforge } => format!(
                "The latest GitHub release ({github}) is not the CurseForge file ({curseforge})."
            ),
            Self::NoRelease(message)
            | Self::NoZip(message)
            | Self::RateLimited(message)
            | Self::Failed(message) => message.clone(),
        }
    }
}

fn unavailable(reason: Reason) -> Outcome {
    Outcome::Unavailable { reason }
}

/// Il mirror di `project` per `file`, il file CurseForge che `curseforge::pick_file` sceglie per
/// l'installazione (stessa regola, stessa opzione della Beta), con lo zip di GitHub per il gioco
/// `flavor` (il flavor dell'installazione, come per `GithubSource::choose`).
///
/// Nessuna richiesta per un progetto che CurseForge non blocca o senza `sourceUrl` GitHub; le
/// altre passano tutte da `github`.
pub(crate) fn resolve<T: PublicTransport>(
    github: &GithubSource<T>,
    project: &Mod,
    file: &File,
    flavor: &str,
) -> Outcome {
    if project.allow_mod_distribution == Some(true) {
        return unavailable(Reason::NotBlocked);
    }
    let Some(repo) = github_repo(project.links.source_url.as_deref()) else {
        return unavailable(Reason::NoGithubSource);
    };
    let release = match github.latest_release(&repo) {
        Ok(release) => release,
        Err(ReleaseError::Limited(message)) => return unavailable(Reason::RateLimited(message)),
        Err(ReleaseError::Missing(message)) => return unavailable(Reason::NoRelease(message)),
        Err(ReleaseError::Other(message)) => return unavailable(Reason::Failed(message)),
    };
    let tag = release.tag_name.clone();
    let different = || {
        unavailable(Reason::DifferentVersion {
            github: tag.clone(),
            curseforge: curseforge_version(file),
        })
    };
    let asset = match github.choose_zip(&release, flavor) {
        Ok(Some(asset)) => asset,
        // Senza zip per il gioco una release diversa resta il motivo più utile: il tag basta a
        // dirlo, anche quando il `release.json` non si legge.
        Ok(None) if !same_version(&tag, None, file) => return different(),
        Err(_) if !same_version(&tag, None, file) => return different(),
        Ok(None) => {
            return unavailable(Reason::NoZip(
                "The latest GitHub release has no single zip for this game version.".to_owned(),
            ));
        }
        Err(ChoiceError::NoFileForGame(message)) => return unavailable(Reason::NoZip(message)),
        Err(ChoiceError::Other(message)) => return unavailable(Reason::Failed(message)),
    };
    if !same_version(&tag, Some(&asset.name), file) {
        return different();
    }
    Outcome::Mirror { repo, tag, asset }
}

/// Il nome con cui il file CurseForge compare nei motivi: il nome dello zip, altrimenti quello
/// mostrato dal sito.
fn curseforge_version(file: &File) -> String {
    file.file_name
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| file.display_name.clone())
}

/// `owner/repo` dal `sourceUrl` di un progetto CurseForge, solo quando punta a
/// `github.com/<owner>/<repo>` (con o senza `www`, `.git`, slash finale, sottopercorsi). Un
/// altro host, un percorso riservato di GitHub o `None` non danno un repository. Nessuna rete.
pub(crate) fn github_repo(source_url: Option<&str>) -> Option<String> {
    let source_url = source_url?.trim();
    if source_url.is_empty() {
        return None;
    }
    install_link::parse_link(source_url)
        .ok()
        .filter(|project| project.source == SourceKind::Github)
        .map(|project| project.key)
}

/// La regola della stessa versione, scelta dell'utente: la release `tag` di GitHub, con il suo
/// zip `github_zip` quando c'è, è il file CurseForge `file` se
///
/// - lo zip ha lo stesso nome del file CurseForge (senza distinguere maiuscole e minuscole),
///   oppure
/// - il tag, senza la `v` iniziale, compare come versione intera nel `fileName` o nel
///   `displayName` del file (vedi [`contains_version`]).
pub(crate) fn same_version(tag: &str, github_zip: Option<&str>, file: &File) -> bool {
    if let (Some(zip), Some(name)) = (github_zip, file.file_name.as_deref())
        && !zip.is_empty()
        && zip.eq_ignore_ascii_case(name)
    {
        return true;
    }
    let tag = tag.trim();
    let version = tag.strip_prefix(['v', 'V']).unwrap_or(tag);
    file.file_name
        .iter()
        .map(String::as_str)
        .chain([file.display_name.as_str()])
        .any(|text| contains_version(text, version))
}

/// Se `version` compare in `text` come versione intera, senza distinguere maiuscole e
/// minuscole. Prima della versione: l'inizio del testo, `-`, `_` o uno spazio, anche seguiti da
/// una `v` (`BigWigs-v427.8.zip`). Dopo: la fine del testo, `_`, uno spazio, `.zip`, oppure `-`
/// non seguito da una cifra. Così `4.18.3.1` non combacia con `4.18.3.10` né con l'Alpha
/// `4.18.3.1-1-g8339c74` (il suffisso di `git describe`).
pub(crate) fn contains_version(text: &str, version: &str) -> bool {
    if version.is_empty() {
        return false;
    }
    let text = text.to_ascii_lowercase();
    let version = version.to_ascii_lowercase();
    let is_delimiter = |c: char| matches!(c, '-' | '_' | ' ');
    (0..text.len())
        .filter(|&start| text.is_char_boundary(start) && text[start..].starts_with(&version))
        .any(|start| {
            let before = &text[..start];
            let starts_cleanly = match before.chars().next_back() {
                None => true,
                Some(c) if is_delimiter(c) => true,
                Some('v') => before[..before.len() - 1]
                    .chars()
                    .next_back()
                    .is_none_or(is_delimiter),
                Some(_) => false,
            };
            let after = &text[start + version.len()..];
            let mut rest = after.chars();
            let ends_cleanly = match rest.next() {
                None => true,
                Some('_' | ' ') => true,
                Some('-') => !rest.next().is_some_and(|c| c.is_ascii_digit()),
                Some('.') => after == ".zip",
                Some(_) => false,
            };
            starts_cleanly && ends_cleanly
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::curseforge::{ModLinks, TransportError};
    use crate::sources::http::{ApiResponse, BytesResponse, HttpResponse};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    const API: &str = "https://api.github.com/repos";

    /// Le risposte del trasporto finto per URL, con le richieste fatte.
    #[derive(Default)]
    struct Fake {
        api: HashMap<String, ApiResponse>,
        json: HashMap<String, String>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl PublicTransport for Fake {
        fn get(&self, url: &str) -> Result<HttpResponse, TransportError> {
            self.calls.lock().unwrap().push(url.to_owned());
            Ok(match self.json.get(url) {
                Some(body) => HttpResponse {
                    status: 200,
                    body: body.clone(),
                },
                None => HttpResponse {
                    status: 404,
                    body: String::new(),
                },
            })
        }

        fn get_api(&self, url: &str, _accept: &str) -> Result<ApiResponse, TransportError> {
            self.calls.lock().unwrap().push(url.to_owned());
            match self.api.get(url) {
                Some(response) => Ok(ApiResponse {
                    status: response.status,
                    body: response.body.clone(),
                    ratelimit_remaining: response.ratelimit_remaining,
                    ratelimit_reset: response.ratelimit_reset,
                }),
                None => Err(TransportError::Connect),
            }
        }

        fn get_bytes(&self, url: &str, _max_len: u64) -> Result<BytesResponse, TransportError> {
            panic!("the mirror downloads nothing: {url}");
        }
    }

    fn downloads(repo: &str, tag: &str) -> String {
        format!("https://github.com/{repo}/releases/download/{tag}")
    }

    /// Una release di `repo` con gli asset `assets` e, se `flavors` non è vuoto, un
    /// `release.json` che assegna il primo zip a quei flavor.
    fn release(fake: &mut Fake, repo: &str, tag: &str, assets: &[&str], flavors: &[&str]) {
        let mut names: Vec<&str> = assets.to_vec();
        if !flavors.is_empty() {
            names.push("release.json");
            let metadata: Vec<_> = flavors
                .iter()
                .map(|flavor| serde_json::json!({"flavor": flavor, "interface": 1}))
                .collect();
            fake.json.insert(
                format!("{}/release.json", downloads(repo, tag)),
                serde_json::json!({"releases": [
                    {"filename": assets[0], "nolib": false, "metadata": metadata}
                ]})
                .to_string(),
            );
        }
        let assets: Vec<_> = names
            .iter()
            .map(|name| {
                serde_json::json!({
                    "name": name,
                    "browser_download_url": format!("{}/{name}", downloads(repo, tag)),
                })
            })
            .collect();
        fake.api.insert(
            format!("{API}/{repo}/releases/latest"),
            ApiResponse {
                status: 200,
                body: serde_json::json!({"tag_name": tag, "assets": assets}).to_string(),
                ratelimit_remaining: Some(59),
                ratelimit_reset: None,
            },
        );
    }

    fn status(fake: &mut Fake, repo: &str, status: u16, remaining: u64, reset: Option<u64>) {
        fake.api.insert(
            format!("{API}/{repo}/releases/latest"),
            ApiResponse {
                status,
                body: String::new(),
                ratelimit_remaining: Some(remaining),
                ratelimit_reset: reset,
            },
        );
    }

    fn project(allow: Option<bool>, source_url: Option<&str>) -> Mod {
        Mod {
            id: 13501,
            name: "Bartender4".to_owned(),
            slug: "bartender4".to_owned(),
            allow_mod_distribution: allow,
            links: ModLinks {
                website_url: None,
                source_url: source_url.map(str::to_owned),
            },
            latest_files_indexes: Vec::new(),
        }
    }

    fn file(file_name: &str, display_name: &str) -> File {
        serde_json::from_value(serde_json::json!({
            "id": 1, "modId": 13501, "displayName": display_name, "fileName": file_name,
            "releaseType": 1,
        }))
        .unwrap()
    }

    fn resolve_with(fake: Fake, project: &Mod, file: &File, flavor: &str) -> Outcome {
        resolve(&GithubSource::new(fake), project, file, flavor)
    }

    fn reason(outcome: Outcome) -> Reason {
        match outcome {
            Outcome::Unavailable { reason } => reason,
            mirror => panic!("expected Unavailable, got {mirror:?}"),
        }
    }

    const BARTENDER: &str = "Nevcairiel/Bartender4";

    #[test]
    fn the_version_rule_follows_the_real_files() {
        let same = |tag: &str, zip: Option<&str>, name: &str, display: &str| {
            same_version(tag, zip, &file(name, display))
        };
        // Bartender4: stesso nome dello zip, e il tag nel nome.
        assert!(same(
            "4.18.3.1",
            Some("Bartender4-4.18.3.1.zip"),
            "Bartender4-4.18.3.1.zip",
            "4.18.3.1"
        ));
        assert!(same(
            "4.18.3.1",
            None,
            "Bartender4-4.18.3.1.zip",
            "Bartender4"
        ));
        // DBM: lo zip di GitHub ha un altro nome, il tag è nel file CurseForge.
        assert!(same(
            "12.1.17",
            Some("DeadlyBossMods-12.1.17.zip"),
            "DBM-Core-12.1.17.zip",
            "DBM"
        ));
        // Solo nel `displayName`.
        assert!(same("12.1.17", None, "dbm.zip", "Deadly Boss Mods 12.1.17"));
        // Una versione più lunga e l'Alpha di `git describe` non sono la stessa versione.
        assert!(!same(
            "4.18.3.1",
            None,
            "Bartender4-4.18.3.10.zip",
            "4.18.3.10"
        ));
        assert!(!same(
            "4.18.3.1",
            None,
            "Bartender4-4.18.3.1-1-g8339c74.zip",
            "4.18.3.1-1-g8339c74"
        ));
        assert!(!same("4.18.3.1", None, "Bartender4-14.18.3.1.zip", ""));
        // La `v` iniziale del tag, le maiuscole, la `v` nel nome del file.
        assert!(same("v12.1.17", None, "DBM-Core-12.1.17.zip", ""));
        assert!(same("V12.1.17", None, "DBM-CORE-12.1.17.ZIP", ""));
        assert!(same("v427.8", None, "BigWigs-v427.8.zip", ""));
        assert!(same(
            "4.18.3.1",
            Some("bartender4-4.18.3.1.ZIP"),
            "Bartender4-4.18.3.1.zip",
            ""
        ));
        // Altri delimitatori.
        assert!(same("1.2", None, "Addon_1.2_classic.zip", ""));
        assert!(same("1.2", None, "Addon-1.2-classic.zip", ""));
        assert!(same("1.2", None, "", "1.2"));
        assert!(!same("1.2", None, "Addon-1.2.3.zip", "Addon1.2"));
        // Un tag vuoto non combacia con niente.
        assert!(!same("v", None, "Addon-v.zip", "v"));
    }

    #[test]
    fn the_repository_comes_only_from_a_github_source_url() {
        let repo = |url: Option<&str>| github_repo(url);
        for url in [
            "https://github.com/Nevcairiel/Bartender4",
            "https://www.github.com/Nevcairiel/Bartender4",
            "https://github.com/Nevcairiel/Bartender4.git",
            "https://github.com/Nevcairiel/Bartender4/",
            "https://github.com/Nevcairiel/Bartender4/tree/master/Options",
            "http://github.com/Nevcairiel/Bartender4",
        ] {
            assert_eq!(repo(Some(url)).as_deref(), Some(BARTENDER), "{url}");
        }
        for url in [
            "https://repos.curseforge.com/wow/tomtom",
            "https://www.curseforge.com/wow/addons/tomtom",
            "https://gitlab.com/owner/repo",
            "https://github.com/Nevcairiel",
            "https://github.com/orgs/Nevcairiel",
            // Host che somigliano a GitHub senza esserlo.
            "https://evilgithub.com/Nevcairiel/Bartender4",
            "https://github.com.evil.example/Nevcairiel/Bartender4",
            "https://api.github.com/repos/Nevcairiel/Bartender4",
            "",
            "   ",
            "not a link",
        ] {
            assert_eq!(repo(Some(url)), None, "{url}");
        }
        // Credenziali nell'URL: l'host vero è quello dopo la chiocciola (composto qui perché il
        // controllo di privacy non lo scambi per un indirizzo email).
        let userinfo = format!(
            "https://github.com{}evil.example/Nevcairiel/Bartender4",
            '@'
        );
        assert_eq!(repo(Some(&userinfo)), None);
        assert_eq!(repo(None), None);
    }

    #[test]
    fn a_matching_release_is_a_mirror() {
        let mut fake = Fake::default();
        release(
            &mut fake,
            BARTENDER,
            "4.18.3.1",
            &["Bartender4-4.18.3.1.zip"],
            &["mainline", "forever"],
        );
        let source = Some("https://github.com/Nevcairiel/Bartender4");
        let outcome = resolve_with(
            fake,
            &project(Some(false), source),
            &file("Bartender4-4.18.3.1.zip", "4.18.3.1"),
            "classic_beta",
        );
        let Outcome::Mirror { repo, tag, asset } = outcome else {
            panic!("expected a mirror, got {outcome:?}");
        };
        assert_eq!(repo, BARTENDER);
        assert_eq!(tag, "4.18.3.1");
        assert_eq!(asset.name, "Bartender4-4.18.3.1.zip");
        assert_eq!(
            asset.browser_download_url,
            format!(
                "{}/Bartender4-4.18.3.1.zip",
                downloads(BARTENDER, "4.18.3.1")
            )
        );

        // DBM: lo zip di GitHub ha un altro nome, il tag decide. `allowModDistribution` assente
        // vale come bloccato.
        let repo = "DeadlyBossMods/DeadlyBossMods";
        let mut fake = Fake::default();
        release(
            &mut fake,
            repo,
            "12.1.17",
            &["DeadlyBossMods-12.1.17.zip"],
            &["forever"],
        );
        let outcome = resolve_with(
            fake,
            &project(
                None,
                Some("https://github.com/DeadlyBossMods/DeadlyBossMods"),
            ),
            &file("DBM-Core-12.1.17.zip", "12.1.17"),
            "classic_beta",
        );
        assert!(
            matches!(&outcome, Outcome::Mirror { tag, .. } if tag == "12.1.17"),
            "{outcome:?}"
        );
    }

    #[test]
    fn projects_that_need_no_mirror_make_no_request() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let fake = || Fake {
            calls: Arc::clone(&calls),
            ..Fake::default()
        };
        let any_file = file("TomTom-v4.0.zip", "v4.0");
        assert_eq!(
            reason(resolve_with(
                fake(),
                &project(Some(true), Some("https://github.com/Nevcairiel/Bartender4")),
                &any_file,
                "retail",
            )),
            Reason::NotBlocked
        );
        for source in [None, Some("https://repos.curseforge.com/wow/tomtom")] {
            assert_eq!(
                reason(resolve_with(
                    fake(),
                    &project(Some(false), source),
                    &any_file,
                    "retail"
                )),
                Reason::NoGithubSource
            );
        }
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn a_missing_release_or_zip_is_unavailable() {
        // Auctionator e Leatrix Plus: nessuna release (404).
        let repo = "Auctionator/Auctionator";
        let mut fake = Fake::default();
        status(&mut fake, repo, 404, 50, None);
        let blocked = project(
            Some(false),
            Some("https://github.com/Auctionator/Auctionator"),
        );
        let reason_404 = reason(resolve_with(
            fake,
            &blocked,
            &file("Auctionator-1.0.zip", "1.0"),
            "retail",
        ));
        assert_eq!(
            reason_404,
            Reason::NoRelease(
                "GitHub has no published release for Auctionator/Auctionator.".into()
            )
        );

        // La release giusta senza zip per il gioco.
        let source = Some("https://github.com/Nevcairiel/Bartender4");
        let mut fake = Fake::default();
        release(
            &mut fake,
            BARTENDER,
            "4.18.3.1",
            &["Bartender4-4.18.3.1.zip"],
            &["mainline"],
        );
        let reason_zip = reason(resolve_with(
            fake,
            &project(Some(false), source),
            &file("Bartender4-4.18.3.1.zip", "4.18.3.1"),
            "classic_beta",
        ));
        assert_eq!(
            reason_zip,
            Reason::NoZip("The latest GitHub release has no file for this game version.".into())
        );

        // Due zip senza `release.json`: nessuno zip da scegliere.
        let mut fake = Fake::default();
        release(
            &mut fake,
            BARTENDER,
            "4.18.3.1",
            &["Bartender4-4.18.3.1.zip", "Bartender4-4.18.3.1-classic.zip"],
            &[],
        );
        assert!(matches!(
            reason(resolve_with(
                fake,
                &project(Some(false), source),
                &file("Bartender4-4.18.3.1.zip", "4.18.3.1"),
                "retail",
            )),
            Reason::NoZip(_)
        ));
    }

    #[test]
    fn an_unreadable_release_json_is_a_failure_not_a_missing_zip() {
        let mut fake = Fake::default();
        release(
            &mut fake,
            BARTENDER,
            "4.18.3.1",
            &["Bartender4-4.18.3.1.zip"],
            &["forever"],
        );
        fake.json.insert(
            format!("{}/release.json", downloads(BARTENDER, "4.18.3.1")),
            "not json".to_owned(),
        );
        assert_eq!(
            reason(resolve_with(
                fake,
                &project(
                    Some(false),
                    Some("https://github.com/Nevcairiel/Bartender4")
                ),
                &file("Bartender4-4.18.3.1.zip", "4.18.3.1"),
                "classic_beta",
            )),
            Reason::Failed("The release.json of the latest GitHub release is unreadable.".into())
        );
    }

    #[test]
    fn a_different_release_names_both_versions() {
        // Details: l'ultima release GitHub è del 2021.
        let repo = "Tercioo/Details-Damage-Meter";
        let mut fake = Fake::default();
        release(
            &mut fake,
            repo,
            "Details.20210101.8241.143",
            &["Details.zip"],
            &[],
        );
        let why = reason(resolve_with(
            fake,
            &project(
                Some(false),
                Some("https://github.com/Tercioo/Details-Damage-Meter"),
            ),
            &file(
                "Details-20261020.14110.169.zip",
                "Details.20261020.14110.169",
            ),
            "retail",
        ));
        assert_eq!(
            why.message(),
            "The latest GitHub release (Details.20210101.8241.143) is not the CurseForge file (Details-20261020.14110.169.zip)."
        );

        // Senza zip per il gioco, una release diversa resta "diversa".
        let mut fake = Fake::default();
        release(
            &mut fake,
            BARTENDER,
            "4.18.3.0",
            &["Bartender4-4.18.3.0.zip"],
            &["mainline"],
        );
        assert!(matches!(
            reason(resolve_with(
                fake,
                &project(
                    Some(false),
                    Some("https://github.com/Nevcairiel/Bartender4")
                ),
                &file("Bartender4-4.18.3.1.zip", "4.18.3.1"),
                "classic_beta",
            )),
            Reason::DifferentVersion { .. }
        ));
    }

    #[test]
    fn the_rate_limit_names_the_reset_time() {
        let mut fake = Fake::default();
        // 2026-10-26 14:32:00 UTC.
        status(&mut fake, BARTENDER, 403, 0, Some(1_793_025_120));
        let why = reason(resolve_with(
            fake,
            &project(
                Some(false),
                Some("https://github.com/Nevcairiel/Bartender4"),
            ),
            &file("Bartender4-4.18.3.1.zip", "4.18.3.1"),
            "retail",
        ));
        assert_eq!(
            why,
            Reason::RateLimited(
                "GitHub's limit of 60 requests an hour has been reached: try again after 14:32 UTC."
                    .into()
            )
        );

        // Un errore di rete non è il limite.
        let why = reason(resolve_with(
            Fake::default(),
            &project(
                Some(false),
                Some("https://github.com/Nevcairiel/Bartender4"),
            ),
            &file("Bartender4-4.18.3.1.zip", "4.18.3.1"),
            "retail",
        ));
        assert_eq!(
            why,
            Reason::Failed("Could not reach GitHub: check the connection.".into())
        );
    }

    #[test]
    fn every_reason_has_its_own_message() {
        let reasons = [
            Reason::NotBlocked,
            Reason::NoGithubSource,
            Reason::DifferentVersion {
                github: "1".into(),
                curseforge: "2".into(),
            },
        ];
        let messages: Vec<String> = reasons.iter().map(Reason::message).collect();
        assert!(messages.iter().all(|m| !m.is_empty()));
        assert_ne!(messages[0], messages[1]);
    }
}
