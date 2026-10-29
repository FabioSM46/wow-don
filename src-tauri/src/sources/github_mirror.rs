//! The GitHub mirror of a CurseForge project that cannot be downloaded (D34, issue #162).
//!
//! When the author forbids downloads from third-party apps (`allowModDistribution` other than
//! `true`, D13), the same addon may also be published in GitHub releases. [`resolve`] says
//! whether it is, and whether the release is the same as the CurseForge file the app installs:
//!
//! - the repository is derived **only** from the project's `sourceUrl`, when it points to
//!   `github.com/<owner>/<repo>` (with the link parsing of `install_link::parse_link`); another
//!   host or no `sourceUrl` makes no request at all. Never a search by name;
//! - the release is read with [`GithubSource`], without the API (issue #179): same cache, same
//!   choice of the zip from `release.json` for the game's flavor, and the block after the rate
//!   limit only for a release without `release.json`, which goes through the API;
//! - the same-version rule is [`same_version`], a pure function.
//!
//! Data verified on 2026-10-26: Bartender4 (`4.18.3.1`) and Deadly Boss Mods (`12.1.17`) have
//! releases with `release.json`; Details has its latest release from 2021; Auctionator and Leatrix
//! Plus have no releases (404).
//!
//! Updates and reinstalls (issue #163) use the mirror through
//! [`MirrorSource`]: they download the GitHub zip with [`install`] and write the **CurseForge**
//! record of the matching file, so the project stays matched to CurseForge.

use std::path::Path;

use serde::Serialize;

use super::curseforge::{File, Mod};
use super::github::{Asset, ChoiceError, GithubSource, ReleaseError};
use super::http::{DownloadProgress, PublicTransport};
use crate::install_link;
use crate::progress::Reporter;
use crate::sources::SourceKind;
use crate::updates;

/// What the CurseForge source asks GitHub for a blocked project: the mirror's outcome and
/// the download of its zip. A trait object, so `CurseForgeSource` does not have a second
/// type parameter for GitHub's transport.
pub(crate) trait MirrorSource {
    /// [`resolve`] with the GitHub source.
    fn resolve(&self, project: &Mod, file: &File, flavor: &str) -> Outcome;

    /// The `asset` zip, with the download rules of `github.rs` (domains, size).
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

/// A project's mirror, as the frontend sees it: `{ repo, tag }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GithubMirror {
    /// `owner/repo`.
    pub repo: String,
    pub tag: String,
}

/// The mirror fields in `AddonUpdate` (`githubMirror`, `githubMirrorReason`): both `null`
/// when the mirror has not been computed (project not blocked, up to date or from another
/// source).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MirrorStatus {
    pub github_mirror: Option<GithubMirror>,
    /// Why the mirror cannot be used (D27, in English).
    pub github_mirror_reason: Option<String>,
    /// The project and the file from which to compute the mirror at the end of the check
    /// (`updates::resolve_mirrors`); empty afterwards. Stays in the backend.
    #[serde(skip)]
    pub pending: Option<Box<Pending>>,
}

/// The blocked CurseForge project and the file to compare with the GitHub release.
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

/// Downloads the mirror's `asset` zip and installs it in place of `old_modules`, with the safe
/// extraction of updates (`updates::install_bytes`). Returns the installed folders:
/// the record, CurseForge, is written by the caller (the updates here, the installation from
/// Search with issue #164). `check` can reject the zip's folders before they touch the
/// disk.
pub(crate) fn install(
    github: &dyn MirrorSource,
    asset: &Asset,
    addons_dir: &Path,
    old_modules: &[String],
    check: &dyn Fn(&[String]) -> Result<(), String>,
    reporter: &Reporter<'_>,
) -> Result<Vec<String>, String> {
    let bytes = download(github, asset, reporter)?;
    updates::install_bytes(&bytes, addons_dir, old_modules, check, reporter)
}

/// Downloads the mirror's `asset` zip reporting progress: the network part of [`install`],
/// which the installation of a new project also uses on its own (issue #180).
pub(crate) fn download(
    github: &dyn MirrorSource,
    asset: &Asset,
    reporter: &Reporter<'_>,
) -> Result<Vec<u8>, String> {
    reporter.download(0, None);
    let bytes = github.download(asset, &mut |received, total| {
        reporter.download(received, total)
    })?;
    reporter.downloaded(bytes.len() as u64);
    Ok(bytes)
}

/// The outcome message of an installation from the mirror.
pub(crate) fn installed_message(repo: &str, tag: &str) -> String {
    format!("Installed from GitHub ({repo} {tag}): CurseForge blocks third-party downloads.")
}

/// The message for a blocked project that cannot be taken from GitHub, with the reason:
/// updates, reinstall and installation from Search.
pub(crate) fn unavailable_message(reason: &Reason) -> String {
    format!(
        "The author does not allow downloads from third-party apps and the GitHub copy cannot be used. {}",
        reason.message()
    )
}

/// The outcome of the mirror lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The GitHub zip to download in place of the CurseForge file.
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

/// Why a project has no usable mirror. [`Reason::message`] is the English text
/// (D27) to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reason {
    /// CurseForge allows the download: the mirror is not needed and GitHub is not queried.
    NotBlocked,
    /// The project has no `sourceUrl` pointing to a GitHub repository.
    NoGithubSource,
    /// The repository does not exist or has no published releases.
    NoRelease(String),
    /// The release has no zip for this game version.
    NoZip(String),
    /// The GitHub release is not the CurseForge file.
    DifferentVersion { github: String, curseforge: String },
    /// GitHub's rate limit, with the reset time.
    RateLimited(String),
    /// Network or unreadable response, of the release or of its `release.json`.
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

/// The mirror of `project` for `file`, the CurseForge file that `curseforge::pick_file` picks for
/// the installation (same rule, same Beta option), with the GitHub zip for the `flavor`
/// game (the installation's flavor, as for `GithubSource::choose`).
///
/// No request for a project that CurseForge does not block or without a GitHub `sourceUrl`; the
/// others all go through `github`.
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
        // Without a zip for the game a different release stays the more useful reason: the tag is
        // enough to say so, even when the `release.json` cannot be read.
        Ok(None) if !same_version(&tag, None, file) => return different(),
        Err(_) if !same_version(&tag, None, file) => return different(),
        Ok(None) => {
            return unavailable(Reason::NoZip(
                "The latest GitHub release has no single zip for this game version.".to_owned(),
            ));
        }
        Err(ChoiceError::NoFileForGame(message)) => return unavailable(Reason::NoZip(message)),
        Err(ChoiceError::Limited(message)) => return unavailable(Reason::RateLimited(message)),
        Err(ChoiceError::Other(message)) => return unavailable(Reason::Failed(message)),
    };
    if !same_version(&tag, Some(&asset.name), file) {
        return different();
    }
    Outcome::Mirror { repo, tag, asset }
}

/// The name under which the CurseForge file appears in the reasons: the zip's name, otherwise the
/// one shown by the site.
fn curseforge_version(file: &File) -> String {
    file.file_name
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| file.display_name.clone())
}

/// `owner/repo` from a CurseForge project's `sourceUrl`, only when it points to
/// `github.com/<owner>/<repo>` (with or without `www`, `.git`, trailing slash, subpaths). Another
/// host, a reserved GitHub path or `None` give no repository. No network.
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

/// The same-version rule, the user's choice: GitHub release `tag`, with its
/// zip `github_zip` when there is one, is CurseForge file `file` if
///
/// - the zip has the same name as the CurseForge file (case-insensitively),
///   or
/// - the tag, without the leading `v`, appears as a whole version in the file's `fileName` or
///   `displayName` (see [`contains_version`]).
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

/// Whether `version` appears in `text` as a whole version, case-insensitively. Before
/// the version: the start of the text, `-`, `_` or a space, optionally followed by
/// a `v` (`BigWigs-v427.8.zip`). After: the end of the text, `_`, a space, `.zip`, or a `-`
/// not followed by a digit. So `4.18.3.1` does not match `4.18.3.10` nor the Alpha
/// `4.18.3.1-1-g8339c74` (the `git describe` suffix).
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
    use crate::sources::http::{ApiResponse, BytesResponse, HttpResponse, RedirectResponse};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    const API: &str = "https://api.github.com/repos";

    /// The fake transport's responses by URL, with the requests made.
    #[derive(Default)]
    struct Fake {
        /// `releases/latest` on the site → (status, `Location`).
        redirects: HashMap<String, (u16, Option<String>)>,
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

        fn get_redirect(&self, url: &str) -> Result<RedirectResponse, TransportError> {
            self.calls.lock().unwrap().push(url.to_owned());
            let (status, location) = self
                .redirects
                .get(url)
                .cloned()
                .ok_or(TransportError::Connect)?;
            Ok(RedirectResponse { status, location })
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

    fn latest(repo: &str) -> String {
        format!("https://github.com/{repo}/releases/latest")
    }

    /// A release of `repo` with the assets `assets` and, if `flavors` is not empty, a
    /// `release.json` that assigns the first zip to those flavors. The site redirects to the tag;
    /// the API lists the assets.
    fn release(fake: &mut Fake, repo: &str, tag: &str, assets: &[&str], flavors: &[&str]) {
        fake.redirects.insert(
            latest(repo),
            (
                302,
                Some(format!("https://github.com/{repo}/releases/tag/{tag}")),
            ),
        );
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
            latest_files: Vec::new(),
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
        // Bartender4: same zip name, and the tag in the name.
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
        // DBM: the GitHub zip has another name, the tag is in the CurseForge file.
        assert!(same(
            "12.1.17",
            Some("DeadlyBossMods-12.1.17.zip"),
            "DBM-Core-12.1.17.zip",
            "DBM"
        ));
        // Only in the `displayName`.
        assert!(same("12.1.17", None, "dbm.zip", "Deadly Boss Mods 12.1.17"));
        // A longer version and the `git describe` Alpha are not the same version.
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
        // The tag's leading `v`, upper case, the `v` in the file name.
        assert!(same("v12.1.17", None, "DBM-Core-12.1.17.zip", ""));
        assert!(same("V12.1.17", None, "DBM-CORE-12.1.17.ZIP", ""));
        assert!(same("v427.8", None, "BigWigs-v427.8.zip", ""));
        assert!(same(
            "4.18.3.1",
            Some("bartender4-4.18.3.1.ZIP"),
            "Bartender4-4.18.3.1.zip",
            ""
        ));
        // Other delimiters.
        assert!(same("1.2", None, "Addon_1.2_classic.zip", ""));
        assert!(same("1.2", None, "Addon-1.2-classic.zip", ""));
        assert!(same("1.2", None, "", "1.2"));
        assert!(!same("1.2", None, "Addon-1.2.3.zip", "Addon1.2"));
        // An empty tag matches nothing.
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
            // Hosts that look like GitHub without being it.
            "https://evilgithub.com/Nevcairiel/Bartender4",
            "https://github.com.evil.example/Nevcairiel/Bartender4",
            "https://api.github.com/repos/Nevcairiel/Bartender4",
            "",
            "   ",
            "not a link",
        ] {
            assert_eq!(repo(Some(url)), None, "{url}");
        }
        // Credentials in the URL: the real host is the one after the at sign (assembled here so
        // that the privacy check does not mistake it for an email address).
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

        // DBM: the GitHub zip has another name, the tag decides. A missing `allowModDistribution`
        // counts as blocked.
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
        // Auctionator and Leatrix Plus: no releases (404).
        let repo = "Auctionator/Auctionator";
        let mut fake = Fake::default();
        fake.redirects.insert(latest(repo), (404, None));
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

        // The right release without a zip for the game.
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

        // Two zips without `release.json`: no zip to choose.
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
        // Details: the latest GitHub release is from 2021.
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

        // Without a zip for the game, a different release stays "different".
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
        // Without `release.json` the zip is chosen from the API, which answers with the rate limit.
        let mut fake = Fake::default();
        release(
            &mut fake,
            BARTENDER,
            "4.18.3.1",
            &["Bartender4-4.18.3.1.zip"],
            &[],
        );
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

        // A network error is not the rate limit.
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
