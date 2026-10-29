//! Reading addon `.toc` files.
//!
//! Reference: the "TOC format" page of Warcraft Wiki (warcraft.wiki.gg), consulted on
//! 2026-10-22. Directives are `## Name: value` lines; `# ...` lines are comments and the
//! others are the addon's file list. The game reads only the first 1024 characters of
//! each line.
//!
//! No I/O here: the parser receives the text, the file choice receives the file names.

use serde::Serialize;

/// The game ignores whatever goes beyond 1024 characters on a line.
const MAX_LINE_CHARS: usize = 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TocInfo {
    /// Already stripped of color codes and textures.
    pub title: Option<String>,
    /// One or more interface version numbers (12.1.5 → 120105).
    pub interface: Vec<u32>,
    pub version: Option<String>,
    pub author: Option<String>,
    pub notes: Option<String>,
    pub dependencies: Vec<String>,
    pub optional_deps: Vec<String>,
    pub load_on_demand: bool,
    /// `## Group`: the folder under which the game groups this addon in its own list
    /// (EllesmereUI's modules declare `EllesmereUI`).
    pub group: Option<String>,
    pub curse_project_id: Option<String>,
    pub wago_id: Option<String>,
    pub wowi_id: Option<String>,
}

/// Parses the text of a `.toc`. It never fails: whatever it does not recognize it ignores.
pub fn parse_toc(text: &str) -> TocInfo {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut info = TocInfo::default();
    for raw in text.lines() {
        let line: String = raw.chars().take(MAX_LINE_CHARS).collect();
        let Some(directive) = line.trim_start().strip_prefix("##") else {
            continue;
        };
        let Some((key, value)) = directive.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match key.as_str() {
            "title" => info.title = non_empty(strip_ui_escapes(value)),
            "interface" => info.interface = parse_interface(value),
            "version" => info.version = Some(value.to_owned()),
            "author" => info.author = non_empty(strip_ui_escapes(value)),
            "notes" => info.notes = non_empty(strip_ui_escapes(value)),
            "optionaldeps" => info.optional_deps = parse_list(value),
            "loadondemand" => info.load_on_demand = value == "1",
            "group" => info.group = non_empty(strip_ui_escapes(value)),
            "x-curse-project-id" => info.curse_project_id = Some(value.to_owned()),
            "x-wago-id" => info.wago_id = Some(value.to_owned()),
            "x-wowi-id" => info.wowi_id = Some(value.to_owned()),
            // "Aliases include RequiredDeps and any word beginning with Dep".
            key if key == "requireddeps" || key.starts_with("dep") => {
                info.dependencies = parse_list(value)
            }
            _ => {}
        }
    }
    info
}

fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn parse_interface(value: &str) -> Vec<u32> {
    value
        .split(',')
        .filter_map(|part| part.trim().parse().ok())
        .collect()
}

fn parse_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Removes the UI escape sequences: colors `|cAARRGGBB…|r` and `|cn<NAME>:…|r`,
/// textures `|T…|t`, atlases `|A…|a`; `||` becomes `|`.
pub fn strip_ui_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '|' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('|') => {
                chars.next();
                out.push('|');
            }
            Some('r') => {
                chars.next();
            }
            Some('c') => {
                chars.next();
                if chars.peek() == Some(&'n') {
                    // |cnCOLOR_NAME:text|r
                    for skipped in chars.by_ref() {
                        if skipped == ':' {
                            break;
                        }
                    }
                } else {
                    for _ in 0..8 {
                        if chars.peek().is_some_and(char::is_ascii_hexdigit) {
                            chars.next();
                        }
                    }
                }
            }
            Some(open @ ('T' | 'A')) => {
                chars.next();
                let close = open.to_ascii_lowercase();
                while let Some(skipped) = chars.next() {
                    if skipped == '|' && chars.peek() == Some(&close) {
                        chars.next();
                        break;
                    }
                }
            }
            _ => out.push('|'),
        }
    }
    out
}

/// `.toc` suffixes to look for for a flavor, from the most specific to the most generic, before
/// the file without a suffix. The flavor is the installation's (`classic_era`); the folder name
/// (`_classic_era_`) is accepted too, and case does not matter.
///
/// The game looks first for the game type's suffix, then the family's (`Mainline` or
/// `Classic`), then the file without a suffix. The `_classic_` folder holds progression
/// Classic (Mists of Pandaria today), `_anniversary_` Classic Anniversary (TBC); WoW Forever
/// (game type `Camelot`, Mainline family) uses `_forever_` for the official client and
/// `_classic_beta_` for the beta.
pub fn toc_suffixes(flavor: &str) -> &'static [&'static str] {
    const RETAIL: &[&str] = &["Standard", "Mainline"];
    const FOREVER: &[&str] = &["Forever", "Camelot", "Mainline"];
    const CLASSIC_ERA: &[&str] = &["Vanilla", "Classic"];
    const CLASSIC_PROGRESSION: &[&str] = &["Mists", "Classic"];
    const ANNIVERSARY: &[&str] = &["TBC", "Classic"];

    match normalize_flavor(flavor).as_str() {
        "classic_beta" | "forever" => FOREVER,
        "anniversary" => ANNIVERSARY,
        f if f.starts_with("classic_era") => CLASSIC_ERA,
        f if f.starts_with("classic") => CLASSIC_PROGRESSION,
        _ => RETAIL,
    }
}

/// `_Classic_Era_` → `classic_era`: the caller can pass the flavor or the folder name.
fn normalize_flavor(flavor: &str) -> String {
    flavor.trim_matches('_').to_ascii_lowercase()
}

/// Hyphenated suffixes used by older clients and still present in many addons.
/// The wiki page documents only the `_` separator: these count as a fallback, after
/// every `_` suffix and before the file without a suffix.
fn legacy_suffixes(flavor: &str) -> &'static [&'static str] {
    let flavor = normalize_flavor(flavor);
    match toc_suffixes(&flavor).last() {
        Some(&"Classic") if flavor == "anniversary" => &["BCC", "Classic"],
        Some(&"Classic") if flavor.starts_with("classic_era") => &["Classic"],
        Some(&"Classic") => &["WOTLKC", "Classic"],
        _ => &["Mainline"],
    }
}

/// Picks the `.toc` of an addon folder for the given flavor. `addon_dir` is the folder's
/// name; the game loads only the `.toc` files that have the same name as the folder.
/// The comparison ignores case, as the game does on Windows, under Wine and on macOS.
pub fn select_toc<'a>(addon_dir: &str, file_names: &'a [String], flavor: &str) -> Option<&'a str> {
    let find = |wanted: String| {
        file_names
            .iter()
            .find(|name| name.eq_ignore_ascii_case(&wanted))
            .map(String::as_str)
    };
    toc_suffixes(flavor)
        .iter()
        .map(|suffix| format!("{addon_dir}_{suffix}.toc"))
        .chain(
            legacy_suffixes(flavor)
                .iter()
                .map(|suffix| format!("{addon_dir}-{suffix}.toc")),
        )
        .chain(std::iter::once(format!("{addon_dir}.toc")))
        .find_map(find)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_simple_toc_is_read() {
        let info = parse_toc(
            "## Interface: 120105\n\
             ## Title: Details! Damage Meter\n\
             ## Notes: Computes damage and healing\n\
             ## Author: Terciob\n\
             ## Version: #Details.20261001\n\
             ## SavedVariables: _detalhes_global\n\
             ## X-Curse-Project-ID: 61284\n\
             ## X-Wago-ID: qv63LLKb\n\
             ## X-WoWI-ID: 23056\n\
             \n\
             # this is a comment\n\
             core.lua\n",
        );
        assert_eq!(info.title.as_deref(), Some("Details! Damage Meter"));
        assert_eq!(info.interface, [120105]);
        assert_eq!(info.notes.as_deref(), Some("Computes damage and healing"));
        assert_eq!(info.author.as_deref(), Some("Terciob"));
        assert_eq!(info.version.as_deref(), Some("#Details.20261001"));
        assert_eq!(info.curse_project_id.as_deref(), Some("61284"));
        assert_eq!(info.wago_id.as_deref(), Some("qv63LLKb"));
        assert_eq!(info.wowi_id.as_deref(), Some("23056"));
        assert!(!info.load_on_demand);
    }

    #[test]
    fn colors_and_textures_are_removed_from_the_title() {
        let info = parse_toc(
            "## Title: |TInterface\\AddOns\\Foo\\icon:16|t |cff33ff99Foo|r |cnGREEN_FONT_COLOR:Bar|r\n",
        );
        assert_eq!(info.title.as_deref(), Some("Foo Bar"));
        assert_eq!(strip_ui_escapes("A||B |A:atlas:16:16|a C"), "A|B  C");
        assert_eq!(strip_ui_escapes("|cFF00ff00Green"), "Green");
        assert_eq!(strip_ui_escapes("end|"), "end|");
    }

    #[test]
    fn multiple_interface_versions_are_read() {
        let info = parse_toc("## Interface: 120105, 50504 ,11509, bogus\n");
        assert_eq!(info.interface, [120105, 50504, 11509]);
    }

    #[test]
    fn bom_crlf_spacing_and_key_case_are_tolerated() {
        let info = parse_toc(
            "\u{feff}##   TITLE  :   Weak Auras  \r\n## loadondemand: 1\r\n## RequiredDeps: Foo, Bar\r\n",
        );
        assert_eq!(info.title.as_deref(), Some("Weak Auras"));
        assert!(info.load_on_demand);
        assert_eq!(info.dependencies, ["Foo", "Bar"]);
    }

    #[test]
    fn dependency_aliases_and_optional_deps() {
        let info = parse_toc("## Dependencies: Core\n## OptionalDeps: Ace3, , LibStub\n");
        assert_eq!(info.dependencies, ["Core"]);
        assert_eq!(info.optional_deps, ["Ace3", "LibStub"]);
        assert_eq!(parse_toc("## DepsCustom: X\n").dependencies, ["X"]);
    }

    #[test]
    fn the_group_field_is_read() {
        let info = parse_toc(
            "## Title: |cff0cd29fEllesmereUI|r Action Bars\n\
             ## Category: |cff0cd29fEllesmere|rUI\n\
             ## Group: EllesmereUI\n\
             ## Dependencies: EllesmereUI\n",
        );
        assert_eq!(info.group.as_deref(), Some("EllesmereUI"));
        assert_eq!(info.dependencies, ["EllesmereUI"]);
        assert_eq!(
            parse_toc("##   group  :   Foo  \r\n").group.as_deref(),
            Some("Foo")
        );
        assert_eq!(
            parse_toc("## Group: |cffff0000Foo|r\n").group.as_deref(),
            Some("Foo")
        );
        assert_eq!(parse_toc("## Group:   \n").group, None);
        assert_eq!(parse_toc("## Group: |cffff0000|r\n").group, None);
        assert_eq!(parse_toc("## Title: Foo\n").group, None);
        // `## Category` is not `## Group`: it stays ignored.
        assert_eq!(parse_toc("## Category: Foo\n").group, None);
    }

    #[test]
    fn broken_and_empty_input_never_panics() {
        assert_eq!(parse_toc(""), TocInfo::default());
        let info = parse_toc(
            "##\n## NoColon\n##:empty key\n## Title:\n## Interface:\n# ## Title: commented\n  ## Title: Indented\n|||c|T\n",
        );
        assert_eq!(info.title.as_deref(), Some("Indented"));
        assert!(info.interface.is_empty());
    }

    #[test]
    fn only_the_first_1024_characters_of_a_line_count() {
        let long = format!("## Notes: {}TAIL\n", "x".repeat(2000));
        let notes = parse_toc(&long).notes.unwrap();
        assert!(!notes.contains("TAIL"));
        assert_eq!(notes.chars().count(), MAX_LINE_CHARS - "## Notes: ".len());
    }

    #[test]
    fn retail_prefers_standard_then_mainline_then_generic() {
        let files = names(&[
            "Foo.toc",
            "Foo_Mainline.toc",
            "Foo_Standard.toc",
            "Foo_Vanilla.toc",
        ]);
        assert_eq!(
            select_toc("Foo", &files, "retail"),
            Some("Foo_Standard.toc")
        );
        let files = names(&["Foo.toc", "Foo_Mainline.toc", "Foo_Vanilla.toc"]);
        assert_eq!(
            select_toc("Foo", &files, "retail"),
            Some("Foo_Mainline.toc")
        );
        assert_eq!(select_toc("Foo", &files, "ptr"), Some("Foo_Mainline.toc"));
    }

    #[test]
    fn classic_flavors_pick_their_game_type_then_family() {
        let files = names(&[
            "Foo.toc",
            "Foo_Classic.toc",
            "Foo_Vanilla.toc",
            "Foo_Mists.toc",
        ]);
        assert_eq!(
            select_toc("Foo", &files, "classic_era"),
            Some("Foo_Vanilla.toc")
        );
        assert_eq!(select_toc("Foo", &files, "classic"), Some("Foo_Mists.toc"));
        assert_eq!(
            select_toc("Foo", &files, "anniversary"),
            Some("Foo_Classic.toc")
        );
        let family_only = names(&["Foo.toc", "Foo_Classic.toc"]);
        assert_eq!(
            select_toc("Foo", &family_only, "classic_era_ptr"),
            Some("Foo_Classic.toc")
        );
    }

    #[test]
    fn forever_uses_its_own_suffix_then_mainline() {
        let files = names(&[
            "Foo.toc",
            "Foo_Mainline.toc",
            "Foo_Forever.toc",
            "Foo_Vanilla.toc",
        ]);
        assert_eq!(
            select_toc("Foo", &files, "classic_beta"),
            Some("Foo_Forever.toc")
        );
        let files = names(&["Foo.toc", "Foo_Mainline.toc", "Foo_Vanilla.toc"]);
        assert_eq!(
            select_toc("Foo", &files, "classic_beta"),
            Some("Foo_Mainline.toc")
        );
    }

    #[test]
    fn the_folder_name_works_as_well_as_the_flavor() {
        let files = names(&[
            "Foo.toc",
            "Foo_Mainline.toc",
            "Foo_Vanilla.toc",
            "Foo-BCC.toc",
        ]);
        assert_eq!(
            select_toc("Foo", &files, "_classic_era_"),
            Some("Foo_Vanilla.toc")
        );
        assert_eq!(
            select_toc("Foo", &files, "Classic_Era"),
            Some("Foo_Vanilla.toc")
        );
        assert_eq!(
            select_toc("Foo", &files, "_retail_"),
            Some("Foo_Mainline.toc")
        );
        assert_eq!(
            select_toc("Foo", &files, "_anniversary_"),
            Some("Foo-BCC.toc")
        );
        assert_eq!(toc_suffixes("_classic_"), toc_suffixes("classic"));
    }

    #[test]
    fn legacy_hyphen_suffixes_are_a_fallback() {
        let files = names(&["Foo.toc", "Foo-Classic.toc", "Foo-BCC.toc"]);
        assert_eq!(
            select_toc("Foo", &files, "classic_era"),
            Some("Foo-Classic.toc")
        );
        assert_eq!(
            select_toc("Foo", &files, "anniversary"),
            Some("Foo-BCC.toc")
        );
        assert_eq!(select_toc("Foo", &files, "retail"), Some("Foo.toc"));
        let both = names(&["Foo-Classic.toc", "Foo_Classic.toc"]);
        assert_eq!(
            select_toc("Foo", &both, "classic_era"),
            Some("Foo_Classic.toc")
        );
    }

    #[test]
    fn generic_toc_is_the_last_resort_and_names_must_match_the_folder() {
        let files = names(&["foo.TOC", "Other.toc", "Foo_Vanilla.toc"]);
        assert_eq!(select_toc("Foo", &files, "retail"), Some("foo.TOC"));
        assert_eq!(select_toc("Foo", &names(&["Other.toc"]), "retail"), None);
        assert_eq!(select_toc("Foo", &[], "retail"), None);
    }
}
