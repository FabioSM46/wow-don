//! Fingerprint di un addon, calcolato come lo calcola CurseForge per riconoscerlo.
//!
//! L'algoritmo, descritto pubblicamente e usato dai client di CurseForge:
//!
//! 1. si scelgono i file della cartella: i `.toc` riconosciuti, i `.lua`/`.xml` che
//!    elencano, gli `<Include file=…/>` e `<Script file=…/>` degli `.xml` in modo ricorsivo,
//!    e `Bindings.xml` nella radice della cartella;
//! 2. ogni file si riduce a un hash MurmurHash2 a 32 bit con seme 1, calcolato saltando i
//!    byte di spaziatura (tab, a capo, ritorno carrello, spazio);
//! 3. gli hash si ordinano in modo crescente, si concatenano in decimale e la stringa
//!    ottenuta si riduce di nuovo con lo stesso hash.
//!
//! Scritto da zero da questa descrizione, senza codice di altri progetti.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use std::rc::Rc;

/// Byte che il calcolo salta: tab, a capo, ritorno carrello, spazio.
fn is_skipped(byte: u8) -> bool {
    matches!(byte, 9 | 10 | 13 | 32)
}

/// MurmurHash2 a 32 bit (Austin Appleby), su un buffer intero. Resta come riferimento dei
/// test: `curse_hash` fa lo stesso calcolo in streaming.
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

/// L'hash di CurseForge: MurmurHash2 con seme 1 sui soli byte che non sono spaziatura.
///
/// Calcolato senza copiare i byte tenuti: la lunghezza filtrata entra nel valore iniziale, quindi
/// una prima passata la conta e la seconda forma le parole da 4 byte saltando la spaziatura. Il
/// risultato è quello di `murmur2` sui byte filtrati (lo verificano i test).
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
    // La coda (1-3 byte) è già in `word` nell'ordine di `murmur2`.
    if filled > 0 {
        h ^= word;
        h = h.wrapping_mul(M);
    }
    h ^= h >> 13;
    h = h.wrapping_mul(M);
    h ^ (h >> 15)
}

/// Combina gli hash dei file nel fingerprint della cartella.
pub fn combine(mut file_hashes: Vec<u32>) -> u32 {
    file_hashes.sort_unstable();
    let joined: String = file_hashes.iter().map(u32::to_string).collect();
    curse_hash(joined.as_bytes())
}

/// I suffissi dei `.toc` che entrano nel calcolo, dopo `_` o `-`. Sono quelli del calcolo
/// di CurseForge, non tutti quelli che il gioco riconosce (vedi `toc::toc_suffixes`): una
/// variante che il calcolo non conosce non deve cambiare il fingerprint.
const TOC_SUFFIXES: &[&str] = &[
    "mainline", "bcc", "tbc", "classic", "vanilla", "wrath", "wotlkc", "cata", "mists",
];

/// I file di un addon, per percorso relativo con `/` e con la cartella dell'addon come primo
/// elemento (`Foo/core.lua`), indicizzati dal percorso in minuscolo. Il gioco su Windows, sotto
/// Wine e su macOS non distingue maiuscole e minuscole, e gli addon contano su questo.
pub type FileIndex = BTreeMap<String, String>;

/// Sceglie i file che entrano nel fingerprint. `read` restituisce il contenuto di un file dato
/// il suo percorso reale (relativo, come nell'indice); il testo si ricava senza copia quando è
/// UTF-8 valido. Restituisce percorsi reali, ordinati per percorso in minuscolo.
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
    // Il calcolo accetta come separatore anche `|`, che in un nome di file non si incontra.
    matches!(separator, Some('_' | '-' | '|')) && TOC_SUFFIXES.contains(&chars.as_str())
}

/// Aggiunge `path` e, ricorsivamente, i file che include.
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
    // I file non sono sempre UTF-8: quello che non lo è diventa un carattere sostitutivo.
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
        // Un nome con caratteri non validi interrompe la lettura delle inclusioni del file,
        // come nel calcolo di CurseForge.
        if name.chars().any(|c| c == '|' || c.is_control()) {
            break;
        }
        if let Some(real) = resolve(dir, &name, files) {
            include(&real, files, read, selected);
        }
    }
}

/// Le inclusioni di un `.toc`: righe che sono un percorso `.lua` o `.xml`, dopo aver tolto i
/// commenti (`#` fino a fine riga).
fn toc_includes(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = match line.find('#') {
                Some(index) => &line[..index],
                None => line,
            }
            .trim();
            // Confronto ASCII al posto di `to_lowercase`, senza un'allocazione per riga: nessun
            // carattere non ASCII diventa `.`, `l`, `u`, `a`, `x` o `m` in minuscolo.
            let wanted = ends_with_ignore_case(line, ".lua") || ends_with_ignore_case(line, ".xml");
            (wanted && !line.contains("..")).then(|| line.to_owned())
        })
        .collect()
}

fn ends_with_ignore_case(text: &str, suffix: &str) -> bool {
    text.len() >= suffix.len()
        && text.as_bytes()[text.len() - suffix.len()..].eq_ignore_ascii_case(suffix.as_bytes())
}

/// Le inclusioni di un `.xml`: il primo `<Include file="…"/>` o `<Script file="…"/>` di ogni
/// riga, dopo aver tolto i commenti `<!-- … -->`.
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

/// Il primo `<Include file=…/>` o `<Script file=…/>` della riga, letto come lo legge il calcolo
/// di CurseForge: il valore va dalla virgoletta di apertura all'**ultima** virgoletta seguita da
/// `/>` sulla riga (se il valore più lungo non è valido si prova il più corto), e non può avere
/// un carattere subito dopo `..`.
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
        // Candidati per la virgoletta di chiusura, dal più lontano al più vicino.
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

/// Vero se nel nome c'è un carattere subito dopo `..` (`..` alla fine è ammesso).
fn char_after_dotdot(name: &str) -> bool {
    name.as_bytes()
        .windows(3)
        .any(|w| w[0] == b'.' && w[1] == b'.')
}

/// Risolve un'inclusione relativa a `dir` nel percorso reale di un file dell'addon.
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

/// Indice di tutti i file sotto `addon_dir`, con percorsi relativi alla cartella `AddOns`.
///
/// I link simbolici a cartelle **non** si seguono: un ciclo (`Foo/loop -> Foo`) bloccherebbe
/// la scansione e un link verso l'esterno farebbe leggere file fuori dall'addon. La cartella
/// dell'addon stessa può essere un link (addon in sviluppo collegati in `AddOns`): si parte
/// da lì. Un link a un file conta come il file.
fn index_files(addon_dir: &Path, folder: &str) -> FileIndex {
    let mut index = FileIndex::new();
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
            if link_meta.is_dir() {
                stack.push((entry.path(), child));
            } else if link_meta.file_type().is_symlink() {
                if fs::metadata(entry.path()).is_ok_and(|target| target.is_file()) {
                    index.insert(child.to_lowercase(), child);
                }
            } else {
                index.insert(child.to_lowercase(), child);
            }
        }
    }
    index
}

/// Il fingerprint della cartella di un addon. `addon_dir` è la cartella in `AddOns`; il suo
/// nome è quello dell'addon. Solo lettura.
///
/// `None` se nessun file entra nel calcolo (cartella vuota, o solo file che il calcolo non
/// conosce): l'hash di un insieme vuoto sarebbe la stessa costante per tutte queste cartelle e
/// potrebbe corrispondere per sbaglio a qualcosa su CurseForge.
///
/// Costo: legge per intero ogni file che entra nel calcolo, una volta sola. Va bene alla
/// cadenza attuale (una scansione quando si apre o si aggiorna un'installazione); se un
/// giorno la scansione diventasse frequente, conviene una cache per cartella.
pub fn folder_fingerprint(addon_dir: &Path) -> Option<u32> {
    let folder = addon_dir.file_name()?.to_str()?;
    let addons_dir = addon_dir.parent()?;
    let files = index_files(addon_dir, folder);
    // I `.toc` e gli `.xml` si leggono per trovare le inclusioni e di nuovo per l'hash: i byte
    // letti la prima volta si riusano, condivisi e non copiati (alcuni `.toc` superano i 50 MB).
    let cache: RefCell<HashMap<String, Option<Rc<[u8]>>>> = RefCell::new(HashMap::new());
    let bytes_of = |relative: &str| {
        cache
            .borrow_mut()
            .entry(relative.to_owned())
            .or_insert_with(|| fs::read(addons_dir.join(relative)).ok().map(Rc::from))
            .clone()
    };
    let selected = select_files(folder, &files, bytes_of);
    let hashes: Vec<u32> = selected
        .iter()
        .filter_map(|relative| bytes_of(relative))
        .map(|bytes| curse_hash(&bytes))
        .collect();
    if hashes.is_empty() {
        return None;
    }
    Some(combine(hashes))
}

/// Il calcolo di prima, che copiava i byte e convertiva i file in `String`: il riferimento dei
/// test di parità sulle cartelle vere.
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

    // Valori attesi calcolati con un'implementazione di riferimento separata di MurmurHash2,
    // scritta in Python dall'algoritmo pubblico di Austin Appleby (non da questo file).
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

    /// Il calcolo di prima, sui byte filtrati copiati in un buffer: il riferimento di
    /// `curse_hash` in streaming.
    fn reference_hash(data: &[u8]) -> u32 {
        let kept: Vec<u8> = data.iter().copied().filter(|b| !is_skipped(*b)).collect();
        murmur2(&kept, 1)
    }

    #[test]
    fn streaming_hash_matches_the_buffered_one_at_every_length() {
        // Contenuti deterministici con spaziatura sparsa: ogni lunghezza filtrata da 0 a oltre
        // 300 byte, quindi ogni resto modulo 4 della coda, con e senza spaziatura in coda.
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
                "## Title: Foo\n# commento\ncore.lua\nui.xml\n",
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
        // Due tag sulla stessa riga: il valore arriva fino all'ultima virgoletta seguita da `/>`,
        // come nel calcolo di riferimento (e quindi non corrisponde a nessun file).
        assert_eq!(
            xml_include_in_line(r#"<Script file="a.lua"/><Script file="b.lua"/>"#).as_deref(),
            Some(r#"a.lua"/><Script file="b.lua"#)
        );
        // Se il valore più lungo contiene `..` seguito da un carattere, si prova il più corto.
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
        // Un ciclo e un link verso l'esterno: nessuno dei due deve essere attraversato.
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
        // Cambiare solo la spaziatura o un file non incluso non cambia il fingerprint.
        fs::write(dir.join("Foo.lua"), "print('a')\r\n\r\n").unwrap();
        fs::write(dir.join("notes.txt"), "changed").unwrap();
        assert_eq!(folder_fingerprint(&dir), Some(expected));
    }
}
