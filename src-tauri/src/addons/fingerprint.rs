//! An addon's fingerprint, computed the way CurseForge computes it to recognize the addon.
//!
//! The algorithm, publicly described and used by CurseForge clients:
//!
//! 1. pick the folder's files: the recognized `.toc` files, the `.lua`/`.xml` files they
//!    list, the `<Include file=…/>` and `<Script file=…/>` of the `.xml` files recursively,
//!    and `Bindings.xml` at the root of the folder;
//! 2. each file is reduced to a 32-bit MurmurHash2 with seed 1, computed skipping the
//!    whitespace bytes (tab, line feed, carriage return, space);
//! 3. the hashes are sorted in ascending order, concatenated in decimal and the resulting
//!    string is reduced again with the same hash.
//!
//! Written from scratch from this description, without code from other projects.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use std::rc::Rc;
use std::time::SystemTime;

/// Bytes the computation skips: tab, line feed, carriage return, space.
fn is_skipped(byte: u8) -> bool {
    matches!(byte, 9 | 10 | 13 | 32)
}

/// 32-bit MurmurHash2 (Austin Appleby), over a whole buffer. It stays as the tests'
/// reference: `curse_hash` does the same computation in streaming.
#[cfg(test)]
pub fn murmur2(data: &[u8], seed: u32) -> u32 {
    const M: u32 = 0x5bd1_e995;
    const R: u32 = 24;
    let mut h = seed ^ data.len() as u32;
    let mut chunks = data.chunks_exact(4);
    for chunk in &mut chunks {
        let mut k = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        k = k.wrapping_mul(M);
        k ^= k >> R;
        k = k.wrapping_mul(M);
        h = h.wrapping_mul(M) ^ k;
    }
    let tail = chunks.remainder();
    if tail.len() >= 3 {
        h ^= u32::from(tail[2]) << 16;
    }
    if tail.len() >= 2 {
        h ^= u32::from(tail[1]) << 8;
    }
    if let Some(&first) = tail.first() {
        h ^= u32::from(first);
        h = h.wrapping_mul(M);
    }
    h ^= h >> 13;
    h = h.wrapping_mul(M);
    h ^ (h >> 15)
}

/// The CurseForge hash: MurmurHash2 with seed 1 over only the bytes that are not whitespace.
///
/// Computed without copying the kept bytes: the filtered length goes into the initial value, so
/// a first pass counts it and the second builds the 4-byte words skipping the whitespace. The
/// result is that of `murmur2` over the filtered bytes (the tests check it).
pub fn curse_hash(data: &[u8]) -> u32 {
    const M: u32 = 0x5bd1_e995;
    const R: u32 = 24;
    let kept = data.iter().filter(|&&b| !is_skipped(b)).count();
    let mut h = 1 ^ kept as u32;
    let mut word = 0u32;
    let mut filled = 0u32;
    for &byte in data {
        if is_skipped(byte) {
            continue;
        }
        word |= u32::from(byte) << (8 * filled);
        filled += 1;
        if filled == 4 {
            let mut k = word.wrapping_mul(M);
            k ^= k >> R;
            k = k.wrapping_mul(M);
            h = h.wrapping_mul(M) ^ k;
            word = 0;
            filled = 0;
        }
    }
    // The tail (1-3 bytes) is already in `word` in `murmur2`'s order.
    if filled > 0 {
        h ^= word;
        h = h.wrapping_mul(M);
    }
    h ^= h >> 13;
    h = h.wrapping_mul(M);
    h ^ (h >> 15)
}

/// Combines the file hashes into the folder's fingerprint.
pub fn combine(mut file_hashes: Vec<u32>) -> u32 {
    file_hashes.sort_unstable();
    let joined: String = file_hashes.iter().map(u32::to_string).collect();
    curse_hash(joined.as_bytes())
}

/// The `.toc` suffixes that go into the computation, after `_` or `-`. They are those of the
/// CurseForge computation, not all those the game recognizes (see `toc::toc_suffixes`): a
/// variant the computation does not know must not change the fingerprint.
const TOC_SUFFIXES: &[&str] = &[
    "mainline", "bcc", "tbc", "classic", "vanilla", "wrath", "wotlkc", "cata", "mists",
];

/// An addon's files, by relative path with `/` and with the addon's folder as the first
/// element (`Foo/core.lua`), indexed by the lowercase path. The game on Windows, under
/// Wine and on macOS ignores case, and addons rely on this.
pub type FileIndex = BTreeMap<String, String>;

/// Picks the files that go into the fingerprint. `read` returns a file's content given its
/// real path (relative, as in the index); the text is derived without a copy when it is
/// valid UTF-8. Returns real paths, sorted by lowercase path.
pub fn select_files<T: AsRef<[u8]>>(
    folder: &str,
    files: &FileIndex,
    read: impl Fn(&str) -> Option<T>,
) -> Vec<String> {
    let folder_lower = folder.to_lowercase();
    let mut selected: Vec<String> = Vec::new();
    let mut tocs: Vec<String> = Vec::new();

    for (lower, real) in files {
        let Some(rest) = lower
            .strip_prefix(&folder_lower)
            .and_then(|r| r.strip_prefix('/'))
        else {
            continue;
        };
        if rest == "bindings.xml" {
            selected.push(real.clone());
        } else if is_counted_toc(&folder_lower, rest) {
            tocs.push(real.clone());
        }
    }
    for toc in &tocs {
        include(toc, files, &read, &mut selected);
    }
    selected.sort_by_key(|path| path.to_lowercase());
    selected
}

fn is_counted_toc(folder_lower: &str, name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".toc") else {
        return false;
    };
    let Some(rest) = stem.strip_prefix(folder_lower) else {
        return false;
    };
    if rest.is_empty() {
        return true;
    }
    let mut chars = rest.chars();
    let separator = chars.next();
    // The computation also accepts `|` as a separator, which never appears in a file name.
    matches!(separator, Some('_' | '-' | '|')) && TOC_SUFFIXES.contains(&chars.as_str())
}

/// Adds `path` and, recursively, the files it includes.
fn include<T: AsRef<[u8]>>(
    path: &str,
    files: &FileIndex,
    read: &impl Fn(&str) -> Option<T>,
    selected: &mut Vec<String>,
) {
    if selected.iter().any(|existing| existing == path) {
        return;
    }
    selected.push(path.to_owned());
    let lower = path.to_lowercase();
    // Files are not always UTF-8: whatever is not becomes a replacement character.
    let includes = if lower.ends_with(".toc") {
        read(path).map(|bytes| toc_includes(&String::from_utf8_lossy(bytes.as_ref())))
    } else if lower.ends_with(".xml") {
        read(path).map(|bytes| xml_includes(&String::from_utf8_lossy(bytes.as_ref())))
    } else {
        None
    };
    let dir = match path.rfind('/') {
        Some(index) => &path[..index],
        None => "",
    };
    for name in includes.unwrap_or_default() {
        // A name with invalid characters stops reading the file's includes,
        // as in the CurseForge computation.
        if name.chars().any(|c| c == '|' || c.is_control()) {
            break;
        }
        if let Some(real) = resolve(dir, &name, files) {
            include(&real, files, read, selected);
        }
    }
}

/// The includes of a `.toc`: lines that are a `.lua` or `.xml` path, after removing the
/// comments (`#` to the end of the line).
fn toc_includes(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = match line.find('#') {
                Some(index) => &line[..index],
                None => line,
            }
            .trim();
            // ASCII comparison instead of `to_lowercase`, without one allocation per line: no
            // non-ASCII character becomes `.`, `l`, `u`, `a`, `x` or `m` in lowercase.
            let wanted = ends_with_ignore_case(line, ".lua") || ends_with_ignore_case(line, ".xml");
            (wanted && !line.contains("..")).then(|| line.to_owned())
        })
        .collect()
}

fn ends_with_ignore_case(text: &str, suffix: &str) -> bool {
    text.len() >= suffix.len()
        && text.as_bytes()[text.len() - suffix.len()..].eq_ignore_ascii_case(suffix.as_bytes())
}

/// The includes of an `.xml`: the first `<Include file="…"/>` or `<Script file="…"/>` of each
/// line, after removing the `<!-- … -->` comments.
fn xml_includes(text: &str) -> Vec<String> {
    let mut without_comments = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        without_comments.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => {
                rest = "";
                break;
            }
        }
    }
    without_comments.push_str(rest);

    without_comments
        .lines()
        .filter_map(|line| xml_include_in_line(line.trim()))
        .collect()
}

/// The first `<Include file=…/>` or `<Script file=…/>` of the line, read the way the CurseForge
/// computation reads it: the value runs from the opening quote to the **last** quote followed by
/// `/>` on the line (if the longest value is not valid the shorter one is tried), and it cannot
/// have a character right after `..`.
fn xml_include_in_line(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let bytes = line.as_bytes();
    for (open, _) in line.match_indices('<') {
        let after_open = &lower[open + 1..];
        let Some(tag_len) = ["include", "script"]
            .iter()
            .find(|tag| after_open.starts_with(*tag))
            .map(|tag| tag.len())
        else {
            continue;
        };
        let mut index = open + 1 + tag_len;
        let spaces = line[index..].len() - line[index..].trim_start().len();
        if spaces == 0 || !lower[index + spaces..].starts_with("file=") {
            continue;
        }
        index += spaces + "file=".len();
        if !matches!(bytes.get(index), Some(b'"' | b'\'')) {
            continue;
        }
        let value_start = index + 1;
        // Candidates for the closing quote, from the farthest to the nearest.
        let mut ends: Vec<usize> = (value_start + 1..line.len())
            .filter(|&i| matches!(bytes[i], b'"' | b'\''))
            .filter(|&i| line[i + 1..].trim_start().starts_with("/>"))
            .collect();
        ends.reverse();
        for end in ends {
            let name = &line[value_start..end];
            if !char_after_dotdot(name) {
                return Some(name.to_owned());
            }
        }
    }
    None
}

/// True if the name has a character right after `..` (`..` at the end is allowed).
fn char_after_dotdot(name: &str) -> bool {
    name.as_bytes()
        .windows(3)
        .any(|w| w[0] == b'.' && w[1] == b'.')
}

/// Resolves an include relative to `dir` to the real path of a file of the addon.
fn resolve(dir: &str, name: &str, files: &FileIndex) -> Option<String> {
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for part in name.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return None,
            part => parts.push(part),
        }
    }
    files.get(&parts.join("/").to_lowercase()).cloned()
}

/// Index of every file under `addon_dir`, with paths relative to the `AddOns` folder.
///
/// Symbolic links to folders are **not** followed: a cycle (`Foo/loop -> Foo`) would block
/// the scan and a link pointing outside would read files outside the addon. The addon's
/// folder itself can be a link (addons in development linked into `AddOns`): the walk starts
/// from there. A link to a file counts as the file.
#[cfg(test)]
fn index_files(addon_dir: &Path, folder: &str) -> FileIndex {
    index_files_stamped(addon_dir, folder).0
}

/// A file of the addon as it is on disk: real path (as in the index), size and modification
/// time. Two equal lists say the fingerprint cannot have changed, without reading the files
/// again (see `addons::cache`).
pub type FileStamp = (String, u64, Option<SystemTime>);

/// Like `index_files`, with each file's size and modification time, sorted by path. For a
/// link to a file, those of the file it points to apply.
fn index_files_stamped(addon_dir: &Path, folder: &str) -> (FileIndex, Vec<FileStamp>) {
    let mut index = FileIndex::new();
    let mut stamps = Vec::new();
    let mut stack = vec![(addon_dir.to_path_buf(), folder.to_owned())];
    while let Some((dir, relative)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let child = format!("{relative}/{name}");
            let Ok(link_meta) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            let file_meta = if link_meta.is_dir() {
                stack.push((entry.path(), child));
                continue;
            } else if link_meta.file_type().is_symlink() {
                match fs::metadata(entry.path()) {
                    Ok(target) if target.is_file() => target,
                    _ => continue,
                }
            } else {
                link_meta
            };
            stamps.push((child.clone(), file_meta.len(), file_meta.modified().ok()));
            index.insert(child.to_lowercase(), child);
        }
    }
    stamps.sort();
    (index, stamps)
}

/// The files of an addon's folder, read once: they serve both to decide whether an already
/// computed fingerprint still holds and to compute it.
pub struct FolderFiles {
    folder: String,
    index: FileIndex,
    pub stamps: Vec<FileStamp>,
}

/// The files of the `addon_dir` folder in `AddOns`; `None` if the name is not a valid path
/// (as for [`folder_fingerprint`]).
pub fn folder_files(addon_dir: &Path) -> Option<FolderFiles> {
    let folder = addon_dir.file_name()?.to_str()?.to_owned();
    let (index, stamps) = index_files_stamped(addon_dir, &folder);
    Some(FolderFiles {
        folder,
        index,
        stamps,
    })
}

/// The fingerprint of an addon's folder. `addon_dir` is the folder in `AddOns`; its name is
/// the addon's. Read-only.
///
/// `None` if no file goes into the computation (empty folder, or only files the computation
/// does not know): the hash of an empty set would be the same constant for all these folders
/// and could match something on CurseForge by mistake.
///
/// Cost: reads every file that goes into the computation in full, only once. The app's scan
/// goes through `addons::cache`, which avoids this when the folder has not changed.
pub fn folder_fingerprint(addon_dir: &Path) -> Option<u32> {
    fingerprint_of(addon_dir, &folder_files(addon_dir)?)
}

/// The fingerprint of `addon_dir` from its already listed files.
pub fn fingerprint_of(addon_dir: &Path, folder_files: &FolderFiles) -> Option<u32> {
    let folder = folder_files.folder.as_str();
    let files = &folder_files.index;
    let addons_dir = addon_dir.parent()?;
    // The `.toc` and `.xml` files are read to find the includes and again for the hash: the
    // bytes read the first time are reused, shared and not copied (some `.toc` exceed 50 MB).
    let cache: RefCell<HashMap<String, Option<Rc<[u8]>>>> = RefCell::new(HashMap::new());
    let bytes_of = |relative: &str| {
        cache
            .borrow_mut()
            .entry(relative.to_owned())
            .or_insert_with(|| fs::read(addons_dir.join(relative)).ok().map(Rc::from))
            .clone()
    };
    let selected = select_files(folder, files, bytes_of);
    // Each file is computed and let go: the `.toc` and `.xml` files read for the includes
    // leave the cache, the `.lua` files are read here and never enter it. One file at a time
    // stays in memory, not the whole addon (the scan computes several folders in parallel).
    let hashes: Vec<u32> = selected
        .iter()
        .filter_map(|relative| match cache.borrow_mut().remove(relative) {
            Some(cached) => cached,
            None => fs::read(addons_dir.join(relative)).ok().map(Rc::from),
        })
        .map(|bytes| curse_hash(&bytes))
        .collect();
    if hashes.is_empty() {
        return None;
    }
    Some(combine(hashes))
}

/// The previous computation, which copied the bytes and converted the files to `String`: the
/// reference for the parity tests on real folders.
#[cfg(test)]
pub fn reference_fingerprint(addon_dir: &Path) -> Option<u32> {
    let folder = addon_dir.file_name()?.to_str()?;
    let addons_dir = addon_dir.parent()?;
    let files = index_files(addon_dir, folder);
    let read = |relative: &str| fs::read(addons_dir.join(relative)).ok();
    let selected = select_files(folder, &files, |relative| {
        read(relative).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    });
    let hashes: Vec<u32> = selected
        .iter()
        .filter_map(|relative| read(relative))
        .map(|bytes| {
            let kept: Vec<u8> = bytes.into_iter().filter(|b| !is_skipped(*b)).collect();
            murmur2(&kept, 1)
        })
        .collect();
    if hashes.is_empty() {
        return None;
    }
    Some(combine(hashes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    // Expected values computed with a separate reference implementation of MurmurHash2,
    // written in Python from Austin Appleby's public algorithm (not from this file).
    #[test]
    fn murmur2_matches_the_reference() {
        for (input, expected) in [
            (&b""[..], 1_540_447_798),
            (b"a", 626_045_324),
            (b"ab", 1_692_487_918),
            (b"abc", 1_621_425_345),
            (b"abcd", 3_376_380_438),
            (b"abcde", 3_469_237_630),
            (b"The quick brown fox jumps over the lazy dog", 504_383_975),
        ] {
            assert_eq!(murmur2(input, 1), expected, "{input:?}");
        }
        assert_eq!(murmur2(b"", 0), 0);
    }

    #[test]
    fn whitespace_is_skipped() {
        assert_eq!(
            curse_hash(b"The quick brown fox jumps over the lazy dog"),
            3_751_777_527
        );
        assert_eq!(curse_hash(b" \t\r\n"), curse_hash(b""));
        assert_eq!(curse_hash(b"a b\r\nc"), curse_hash(b"abc"));
    }

    /// The previous computation, over the filtered bytes copied into a buffer: the reference
    /// for the streaming `curse_hash`.
    fn reference_hash(data: &[u8]) -> u32 {
        let kept: Vec<u8> = data.iter().copied().filter(|b| !is_skipped(*b)).collect();
        murmur2(&kept, 1)
    }

    #[test]
    fn streaming_hash_matches_the_buffered_one_at_every_length() {
        // Deterministic contents with scattered whitespace: every filtered length from 0 to over
        // 300 bytes, hence every tail remainder modulo 4, with and without trailing whitespace.
        let mut state = 0x1234_5678u32;
        for len in 0..=320 {
            let data: Vec<u8> = (0..len)
                .map(|_| {
                    state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                    match state >> 28 {
                        0 => b' ',
                        1 => b'\n',
                        2 => b'\t',
                        3 => b'\r',
                        _ => (state >> 16) as u8,
                    }
                })
                .collect();
            assert_eq!(curse_hash(&data), reference_hash(&data), "len {len}");
            let plain: Vec<u8> = data.iter().copied().filter(|b| !is_skipped(*b)).collect();
            assert_eq!(curse_hash(&plain), murmur2(&plain, 1), "plain len {len}");
        }
    }

    #[test]
    fn ascii_suffix_check_matches_lowercase() {
        for line in [
            "Core.LUA",
            "a.Xml",
            "x.lua ",
            "ä.lua",
            "İ.LUA",
            "\u{212A}.lua",
            "lua",
            ".lu",
            "",
            "Ω.XmL",
            "file.luac",
        ] {
            let lower = line.to_lowercase();
            assert_eq!(
                ends_with_ignore_case(line, ".lua") || ends_with_ignore_case(line, ".xml"),
                lower.ends_with(".lua") || lower.ends_with(".xml"),
                "{line:?}"
            );
        }
    }

    #[test]
    fn combine_sorts_and_concatenates() {
        let a = curse_hash(b"print('a')\n");
        let b = curse_hash(b"## Title: Foo\nFoo.lua\n");
        assert_eq!(combine(vec![a, b]), 2_003_240_298);
        assert_eq!(combine(vec![b, a]), combine(vec![a, b]));
    }

    fn index(paths: &[&str]) -> FileIndex {
        paths
            .iter()
            .map(|p| (p.to_lowercase(), (*p).to_owned()))
            .collect()
    }

    fn reader<'a>(contents: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |path| {
            contents
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, c)| (*c).to_owned())
        }
    }

    #[test]
    fn toc_lua_xml_includes_and_bindings_are_selected() {
        let files = index(&[
            "Foo/Foo.toc",
            "Foo/core.lua",
            "Foo/ui.xml",
            "Foo/widgets/button.lua",
            "Foo/widgets/frame.xml",
            "Foo/Bindings.xml",
            "Foo/unused.lua",
            "Foo/README.md",
        ]);
        let contents = [
            (
                "Foo/Foo.toc",
                "## Title: Foo\n# comment\ncore.lua\nui.xml\n",
            ),
            (
                "Foo/ui.xml",
                "<Ui>\n  <Include file=\"widgets\\frame.xml\"/>\n  <!-- <Script file=\"unused.lua\"/> -->\n</Ui>\n",
            ),
            (
                "Foo/widgets/frame.xml",
                "<Ui>\n<Script file='button.lua' />\n</Ui>\n",
            ),
        ];
        let selected = select_files("Foo", &files, reader(&contents));
        assert_eq!(
            selected,
            [
                "Foo/Bindings.xml",
                "Foo/core.lua",
                "Foo/Foo.toc",
                "Foo/ui.xml",
                "Foo/widgets/button.lua",
                "Foo/widgets/frame.xml",
            ]
        );
    }

    #[test]
    fn names_resolve_without_case_and_unsafe_includes_are_ignored() {
        let files = index(&["Foo/Foo.toc", "Foo/Core.LUA", "Bar/evil.lua"]);
        let contents = [(
            "Foo/Foo.toc",
            "core.lua\n../Bar/evil.lua\nmissing.lua\nnot-code.txt\n",
        )];
        let selected = select_files("Foo", &files, reader(&contents));
        assert_eq!(selected, ["Foo/Core.LUA", "Foo/Foo.toc"]);
    }

    #[test]
    fn counted_toc_variants() {
        let files = index(&[
            "Foo/Foo.toc",
            "Foo/Foo_Vanilla.toc",
            "Foo/Foo-Classic.toc",
            "Foo/Foo_Standard.toc",
            "Foo/Other.toc",
        ]);
        let selected = select_files("Foo", &files, |_| None::<String>);
        assert_eq!(
            selected,
            ["Foo/Foo-Classic.toc", "Foo/Foo.toc", "Foo/Foo_Vanilla.toc"]
        );
    }

    #[test]
    fn a_control_character_stops_the_includes_of_that_file() {
        let files = index(&["Foo/Foo.toc", "Foo/a.lua", "Foo/b.lua"]);
        let contents = [("Foo/Foo.toc", "a.lua\nbad\u{1}.lua\nb.lua\n")];
        let selected = select_files("Foo", &files, reader(&contents));
        assert_eq!(selected, ["Foo/a.lua", "Foo/Foo.toc"]);
    }

    #[test]
    fn xml_include_parsing() {
        assert_eq!(
            xml_include_in_line(r#"<Include file="a.xml"/>"#).as_deref(),
            Some("a.xml")
        );
        assert_eq!(
            xml_include_in_line(r#"<script  file='b.lua' />"#).as_deref(),
            Some("b.lua")
        );
        assert_eq!(xml_include_in_line(r#"<Script file="c.lua">"#), None);
        assert_eq!(xml_include_in_line(r#"<Scriptfile="c.lua"/>"#), None);
        assert_eq!(xml_include_in_line(r#"<Include file="../x.xml"/>"#), None);
        assert_eq!(xml_include_in_line("<Frame name=\"x\"/>"), None);
        // Two tags on the same line: the value runs up to the last quote followed by `/>`,
        // as in the reference computation (and so it matches no file).
        assert_eq!(
            xml_include_in_line(r#"<Script file="a.lua"/><Script file="b.lua"/>"#).as_deref(),
            Some(r#"a.lua"/><Script file="b.lua"#)
        );
        // If the longest value contains `..` followed by a character, the shorter one is tried.
        assert_eq!(
            xml_include_in_line(r#"<Script file="a.lua"/><Script file="../b.lua"/>"#).as_deref(),
            Some("a.lua")
        );
    }

    #[test]
    fn a_folder_with_nothing_to_count_has_no_fingerprint() {
        let tmp = TempDir::new();
        let dir = tmp.path().join("Interface/AddOns/Foo");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(folder_fingerprint(&dir), None);
        fs::write(dir.join("Foo_Standard.toc"), "## Title: Foo\n").unwrap();
        fs::write(dir.join("readme.txt"), "x").unwrap();
        assert_eq!(folder_fingerprint(&dir), None);
    }

    #[cfg(unix)]
    #[test]
    fn directory_links_are_not_followed() {
        let tmp = TempDir::new();
        let dir = tmp.path().join("Interface/AddOns/Foo");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.lua"), "x").unwrap();
        // A cycle and a link pointing outside: neither must be traversed.
        std::os::unix::fs::symlink(&dir, dir.join("loop")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("ext")).unwrap();
        fs::write(
            dir.join("Foo.toc"),
            "Foo.lua\next/secret.lua\nloop/Foo.lua\n",
        )
        .unwrap();
        fs::write(dir.join("Foo.lua"), "print('a')\n").unwrap();

        let index = index_files(&dir, "Foo");
        assert!(
            index
                .keys()
                .all(|k| !k.contains("secret") && !k.contains("loop"))
        );
        let expected = combine(vec![
            curse_hash(b"Foo.lua\next/secret.lua\nloop/Foo.lua\n"),
            curse_hash(b"print('a')\n"),
        ]);
        assert_eq!(folder_fingerprint(&dir), Some(expected));
    }

    #[test]
    fn folder_fingerprint_reads_the_disk_and_is_stable() {
        let tmp = TempDir::new();
        let dir = tmp.path().join("Interface/AddOns/Foo");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Foo.toc"), "## Title: Foo\nFoo.lua\n").unwrap();
        fs::write(dir.join("Foo.lua"), "print('a')\n").unwrap();
        fs::write(dir.join("notes.txt"), "not counted").unwrap();

        let expected = combine(vec![
            curse_hash(b"## Title: Foo\nFoo.lua\n"),
            curse_hash(b"print('a')\n"),
        ]);
        assert_eq!(folder_fingerprint(&dir), Some(expected));
        // Changing only whitespace or a file that is not included does not change the fingerprint.
        fs::write(dir.join("Foo.lua"), "print('a')\r\n\r\n").unwrap();
        fs::write(dir.join("notes.txt"), "changed").unwrap();
        assert_eq!(folder_fingerprint(&dir), Some(expected));
    }
}
