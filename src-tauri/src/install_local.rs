//! Installation of a zip the user downloaded by hand from the website (D32), for the addons the
//! app cannot download on its own (for example `allowModDistribution = false`, D13).
//!
//! Path-only functions: no network, no Tauri. The flow is the same as the other callers of
//! [`install`]: `WorkDir::staging` → `extract` → `.toc` check → `replace`.
//! It writes no record to `installed-addons.json`: the addon is then recognised by its
//! fingerprint, like one installed by hand. The `install_zip` command then removes the records of
//! the projects whose folders it replaced (`InstalledAddonsStore::forget_folders`, issue #117).

use std::collections::HashSet;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use serde::Serialize;

use crate::install;
use crate::progress::{Phase, ProgressSink, Reporter};
use crate::sources::curseforge::MAX_DOWNLOAD_BYTES;

/// Maximum size of the zip chosen by the user: the same as a downloaded zip.
pub const MAX_ZIP_BYTES: u64 = MAX_DOWNLOAD_BYTES;

/// Outcome of an installation from a local zip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalInstall {
    /// The folders of the zip, now in `AddOns`, in the zip's order.
    pub installed: Vec<String>,
    /// The folders that already existed with the same name (ignoring case) and were replaced,
    /// with the name they had on disk.
    pub replaced: Vec<String>,
    /// A warning on a successful installation: for example the app's records that could not be
    /// updated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// Installs the zip at `zip_path` into `addons_dir`. The whole zip is rejected, without touching
/// `AddOns`, if it is not a `.zip` file, exceeds [`MAX_ZIP_BYTES`], fails the checks of
/// [`install::extract`] or has a top-level folder without a `.toc` named after it.
/// Folders with the same name that are already present are replaced without confirmation (D32).
///
/// `progress` receives the `extracting` and `installing` phases, with no source and no project: a
/// zip chosen by the user has neither.
pub fn install_zip(
    addons_dir: &Path,
    zip_path: &Path,
    progress: &ProgressSink<'_>,
) -> Result<LocalInstall, String> {
    let reporter = Reporter::new(progress, None, "", "");
    reporter.phase(Phase::Extracting);
    let bytes = read_zip(zip_path)?;
    let staging = install::WorkDir::staging(addons_dir)?;
    let folders = install::extract(&bytes, staging.path())?;
    for folder in &folders {
        if !has_own_toc(&staging.path().join(folder), folder)? {
            return Err(format!(
                "The zip is not a valid addon: the folder {folder} has no {folder}.toc file."
            ));
        }
    }
    let replaced = existing_folders(addons_dir, &folders)?;
    reporter.phase(Phase::Installing);
    install::replace(addons_dir, staging.path(), &[])?;
    Ok(LocalInstall {
        installed: folders,
        replaced,
        warning: None,
    })
}

/// Reads the zip, with the checks on type, extension and size. The messages do not contain the
/// path: the user knows which file they chose.
fn read_zip(zip_path: &Path) -> Result<Vec<u8>, String> {
    let is_zip = zip_path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"));
    if !is_zip {
        return Err("Only .zip files can be installed.".to_string());
    }
    let metadata = fs::metadata(zip_path).map_err(open_error)?;
    if !metadata.is_file() {
        return Err("The chosen item is not a file.".to_string());
    }
    if metadata.len() > MAX_ZIP_BYTES {
        return Err(too_big());
    }
    let file = fs::File::open(zip_path).map_err(open_error)?;
    let mut bytes = Vec::new();
    // The file can grow between the check and the read: we never read past the limit.
    file.take(MAX_ZIP_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(open_error)?;
    if bytes.len() as u64 > MAX_ZIP_BYTES {
        return Err(too_big());
    }
    Ok(bytes)
}

fn too_big() -> String {
    format!(
        "The zip is larger than the {} MiB limit.",
        MAX_ZIP_BYTES / (1024 * 1024)
    )
}

fn open_error(error: io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => "The zip file was not found.".to_string(),
        kind => format!("Could not read the zip file ({kind})."),
    }
}

/// Whether `dir` contains a `.toc` file the game would load for the folder `folder`:
/// `<folder>.toc`, or `<folder>_<suffix>.toc` or `<folder>-<suffix>.toc` (see
/// `addons::toc::select_toc`), ignoring case. It need not be the `.toc` of the installation's
/// flavor.
fn has_own_toc(dir: &Path, folder: &str) -> Result<bool, String> {
    let read_error = |error: io::Error| {
        format!(
            "Could not read the extracted folder {folder} ({}).",
            error.kind()
        )
    };
    let folder = folder.to_lowercase();
    for entry in fs::read_dir(dir).map_err(read_error)? {
        let entry = entry.map_err(read_error)?;
        if !entry.file_type().map_err(read_error)?.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_lowercase) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".toc") else {
            continue;
        };
        let own = stem == folder
            || stem
                .strip_prefix(folder.as_str())
                .is_some_and(|rest| rest.len() > 1 && rest.starts_with(['_', '-']));
        if own {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The entries of `addons_dir` that `replace` will replace, with the name they have on disk.
fn existing_folders(addons_dir: &Path, folders: &[String]) -> Result<Vec<String>, String> {
    let wanted: HashSet<String> = folders.iter().map(|name| name.to_lowercase()).collect();
    let entries = match fs::read_dir(addons_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "Could not read the AddOns folder ({}).",
                error.kind()
            ));
        }
    };
    let mut replaced = Vec::new();
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("Could not read the AddOns folder ({}).", error.kind()))?;
        if let Ok(name) = entry.file_name().into_string()
            && wanted.contains(&name.to_lowercase())
        {
            replaced.push(name);
        }
    }
    replaced.sort();
    Ok(replaced)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress;
    use crate::test_support::TempDir;
    use std::io::{Cursor, Write};
    use std::path::PathBuf;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, text) in files {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(text.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    /// `Interface/AddOns` with spaces in the path, as in Wine prefixes, and the user's downloads
    /// folder.
    struct Fixture {
        _tmp: TempDir,
        addons: PathBuf,
        downloads: PathBuf,
    }

    fn fixture() -> Fixture {
        let tmp = TempDir::new();
        let addons = tmp
            .path()
            .join("Program Files (x86)")
            .join("World of Warcraft")
            .join("_retail_")
            .join("Interface")
            .join("AddOns");
        let downloads = tmp.path().join("My Downloads");
        fs::create_dir_all(&addons).unwrap();
        fs::create_dir_all(&downloads).unwrap();
        Fixture {
            _tmp: tmp,
            addons,
            downloads,
        }
    }

    impl Fixture {
        fn zip(&self, name: &str, files: &[(&str, &str)]) -> PathBuf {
            let path = self.downloads.join(name);
            fs::write(&path, zip_of(files)).unwrap();
            path
        }
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    /// The only entries next to `AddOns`: no leftover work folder.
    fn assert_no_leftovers(fx: &Fixture) {
        assert_eq!(listing(fx.addons.parent().unwrap()), ["AddOns"]);
    }

    #[test]
    fn installs_a_single_addon() {
        let fx = fixture();
        let zip = fx.zip(
            "Details.zip",
            &[
                ("Details/Details.toc", "## Title: Details\n"),
                ("Details/core.lua", "-- code\n"),
            ],
        );
        let collected = progress::tests::Collected::default();
        let result = install_zip(&fx.addons, &zip, &collected.sink()).unwrap();
        // A zip chosen by the user has no source and no download.
        assert_eq!(
            collected.phases(),
            [
                (String::new(), progress::Phase::Extracting),
                (String::new(), progress::Phase::Installing)
            ]
        );
        assert!(collected.0.borrow().iter().all(|p| p.source.is_none()));
        assert_eq!(
            result,
            LocalInstall {
                installed: vec!["Details".to_string()],
                replaced: vec![],
                warning: None,
            }
        );
        assert_eq!(
            fs::read_to_string(fx.addons.join("Details").join("core.lua")).unwrap(),
            "-- code\n"
        );
        assert_no_leftovers(&fx);
    }

    #[test]
    fn installs_several_folders_with_flavor_tocs() {
        let fx = fixture();
        let zip = fx.zip(
            "Pack.ZIP",
            &[
                ("Pack/Pack_Mainline.toc", ""),
                ("Pack_Options/Pack_Options-Classic.toc", ""),
                ("Pack_Config/pack_config.TOC", ""),
            ],
        );
        let result = install_zip(&fx.addons, &zip, &progress::ignore).unwrap();
        assert_eq!(result.installed, ["Pack", "Pack_Options", "Pack_Config"]);
        assert_eq!(listing(&fx.addons), ["Pack", "Pack_Config", "Pack_Options"]);
    }

    #[test]
    fn replaces_existing_folders_ignoring_letter_case() {
        let fx = fixture();
        fs::create_dir_all(fx.addons.join("details")).unwrap();
        fs::write(fx.addons.join("details").join("old.lua"), "old").unwrap();
        fs::create_dir_all(fx.addons.join("Other")).unwrap();
        let zip = fx.zip(
            "Details.zip",
            &[("Details/Details.toc", ""), ("Details/new.lua", "new")],
        );
        let result = install_zip(&fx.addons, &zip, &progress::ignore).unwrap();
        assert_eq!(result.replaced, ["details"]);
        assert_eq!(listing(&fx.addons), ["Details", "Other"]);
        assert_eq!(
            listing(&fx.addons.join("Details")),
            ["Details.toc", "new.lua"]
        );
        assert_no_leftovers(&fx);
    }

    #[test]
    fn rejects_a_folder_without_its_own_toc() {
        let fx = fixture();
        fs::create_dir_all(fx.addons.join("Good")).unwrap();
        let cases: &[&[(&str, &str)]] = &[
            // One valid folder and one without a `.toc`.
            &[("Good/Good.toc", ""), ("Libs/lib.lua", "")],
            // The `.toc` has the name of another folder.
            &[("Good/Other.toc", "")],
            // A prefix that is not followed by `_` or `-`.
            &[("Good/Goodies.toc", "")],
            // An empty suffix.
            &[("Good/Good_.toc", "")],
            // The `.toc` is in a subfolder.
            &[("Good/sub/Good.toc", "")],
            // One extra container folder.
            &[("Good-v1/Good/Good.toc", "")],
        ];
        for files in cases {
            let zip = fx.zip("bad.zip", files);
            let error = install_zip(&fx.addons, &zip, &progress::ignore).unwrap_err();
            assert!(error.contains("not a valid addon"), "{files:?}: {error}");
            assert_eq!(listing(&fx.addons), ["Good"], "{files:?}");
            assert!(listing(&fx.addons.join("Good")).is_empty(), "{files:?}");
            assert_no_leftovers(&fx);
        }
    }

    #[test]
    fn rejects_what_extract_rejects() {
        let fx = fixture();
        let zip = fx.zip("loose.zip", &[("readme.txt", "hi")]);
        let error = install_zip(&fx.addons, &zip, &progress::ignore).unwrap_err();
        assert!(error.contains("outside a folder"), "{error}");
        assert!(listing(&fx.addons).is_empty());
        assert_no_leftovers(&fx);

        // A valid zip with no entries installs nothing: `extract` rejects it, so the `.toc` check
        // cannot pass vacuously.
        let empty = fx.zip("empty.zip", &[]);
        assert_eq!(
            install_zip(&fx.addons, &empty, &progress::ignore).unwrap_err(),
            "The zip is empty."
        );
        assert!(listing(&fx.addons).is_empty());
        assert_no_leftovers(&fx);
    }

    #[test]
    fn rejects_files_that_are_not_zips() {
        let fx = fixture();
        let rar = fx.downloads.join("Details.rar");
        fs::write(&rar, zip_of(&[("Details/Details.toc", "")])).unwrap();
        assert_eq!(
            install_zip(&fx.addons, &rar, &progress::ignore).unwrap_err(),
            "Only .zip files can be installed."
        );

        let fake = fx.downloads.join("fake.zip");
        fs::write(&fake, "not a zip").unwrap();
        assert!(
            install_zip(&fx.addons, &fake, &progress::ignore)
                .unwrap_err()
                .contains("not a valid zip")
        );

        let dir = fx.downloads.join("folder.zip");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            install_zip(&fx.addons, &dir, &progress::ignore).unwrap_err(),
            "The chosen item is not a file."
        );

        assert_eq!(
            install_zip(
                &fx.addons,
                &fx.downloads.join("missing.zip"),
                &progress::ignore
            )
            .unwrap_err(),
            "The zip file was not found."
        );
        assert!(listing(&fx.addons).is_empty());
        assert_no_leftovers(&fx);
    }

    #[test]
    fn rejects_a_zip_over_the_limit() {
        let fx = fixture();
        let big = fx.downloads.join("big.zip");
        let file = fs::File::create(&big).unwrap();
        // A sparse file: no byte actually written.
        file.set_len(MAX_ZIP_BYTES + 1).unwrap();
        let error = install_zip(&fx.addons, &big, &progress::ignore).unwrap_err();
        assert!(error.contains("larger than"), "{error}");
        assert!(listing(&fx.addons).is_empty());
    }
}
