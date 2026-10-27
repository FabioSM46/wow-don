//! Dall'HTML della descrizione di un addon al testo semplice.
//!
//! È una difesa: l'HTML scritto da un autore, mostrato nella webview, avrebbe accesso a
//! `invoke()`. Al frontend arriva solo testo: niente tag, il contenuto di `<script>` e
//! `<style>` scartato, le entità comuni decodificate. I paragrafi e i blocchi diventano una
//! riga vuota, `<br>` e `<li>` un a capo, gli spazi ripetuti uno solo. Non è un parser HTML
//! completo e non serve che lo sia: quello che non riconosce lo scarta o lo lascia come testo.
//!
//! **Il risultato è testo, non HTML sicuro.** Decodificare le entità rimette in chiaro i
//! caratteri del markup: `&lt;img src=x onerror=...&gt;` diventa la stringa
//! `<img src=x onerror=...>`. È corretto finché resta testo, quindi il contratto con il
//! frontend è che `description` si mostra **solo come testo** (il binding di testo di React,
//! `{description}`, o `textContent`), **mai** con `innerHTML` o `dangerouslySetInnerHTML`.
//! Qui non si fa l'escape di `<`, `>` e `&`: il binding di testo lo fa già, e un escape doppio
//! mostrerebbe `&lt;3` invece di `<3`.

/// Il testo semplice di un frammento HTML. Va mostrato come testo, mai come HTML (vedi il
/// modulo).
pub fn plain_text(html: &str) -> String {
    let mut out = Writer::default();
    let mut rest = html;
    while let Some(c) = rest.chars().next() {
        match c {
            '<' => rest = tag(rest, &mut out),
            '&' => {
                let (decoded, len) = entity(rest);
                match decoded {
                    Some(decoded) => out.push(decoded),
                    None => out.push('&'),
                }
                rest = &rest[len..];
            }
            _ => {
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    out.out
}

/// Il testo prodotto, con gli spazi e gli a capo ancora da scrivere: si scrivono solo prima
/// del carattere successivo, così non restano mai all'inizio o alla fine.
#[derive(Default)]
struct Writer {
    out: String,
    space: bool,
    /// 0 niente, 1 un a capo, 2 una riga vuota.
    breaks: u8,
}

impl Writer {
    fn push(&mut self, c: char) {
        if c.is_whitespace() {
            self.space = true;
            return;
        }
        if !self.out.is_empty() {
            match self.breaks {
                0 if self.space => self.out.push(' '),
                0 => {}
                1 => self.out.push('\n'),
                _ => self.out.push_str("\n\n"),
            }
        }
        self.space = false;
        self.breaks = 0;
        self.out.push(c);
    }

    fn line_break(&mut self, level: u8) {
        self.breaks = self.breaks.max(level);
    }
}

/// Elabora il tag all'inizio di `rest` e restituisce quello che segue.
fn tag<'a>(rest: &'a str, out: &mut Writer) -> &'a str {
    if let Some(comment) = rest.strip_prefix("<!--") {
        return comment.find("-->").map_or("", |end| &comment[end + 3..]);
    }
    // `a < b` è testo: un tag inizia con una lettera, `/`, `!` o `?`.
    let opens_tag = rest[1..]
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?'));
    if !opens_tag {
        out.push('<');
        return &rest[1..];
    }
    // Un tag non chiuso alla fine del testo si scarta con quello che segue.
    let Some(end) = tag_end(rest) else {
        return "";
    };
    let inner = &rest[1..end];
    let after = &rest[end + 1..];
    let closing = inner.starts_with('/');
    let name = inner
        .trim_start_matches(['/', '!', '?'])
        .split(|c: char| !c.is_ascii_alphanumeric())
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match name.as_str() {
        "script" | "style" if !closing => skip_raw(after, &name),
        "br" => {
            out.line_break((out.breaks + 1).min(2));
            after
        }
        "li" | "tr" | "dt" | "dd" => {
            out.line_break(1);
            after
        }
        "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol" | "dl" | "table"
        | "blockquote" | "pre" | "hr" | "section" | "article" | "header" | "footer" | "figure" => {
            out.line_break(2);
            after
        }
        "td" | "th" => {
            out.push(' ');
            after
        }
        _ => after,
    }
}

/// L'indice del `>` che chiude il tag all'inizio di `rest`, senza contare quelli tra virgolette
/// negli attributi.
fn tag_end(rest: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in rest.char_indices().skip(1) {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), _) if c == q => quote = None,
            (None, '>') => return Some(i),
            _ => {}
        }
    }
    None
}

/// Salta il contenuto di `<script>` o `<style>` fino al tag di chiusura compreso; senza
/// chiusura scarta tutto il resto.
fn skip_raw<'a>(rest: &'a str, name: &str) -> &'a str {
    // `to_ascii_lowercase` non sposta i byte: gli indici valgono anche per `rest`.
    let lower = rest.to_ascii_lowercase();
    let Some(start) = lower.find(&format!("</{name}")) else {
        return "";
    };
    lower[start..]
        .find('>')
        .map_or("", |end| &rest[start + end + 1..])
}

/// L'entità all'inizio di `rest` (che inizia con `&`) e quanti byte occupa. Un'entità che non
/// riconosce vale come un `&` letterale.
fn entity(rest: &str) -> (Option<char>, usize) {
    let not_entity = (None, 1);
    let Some(semi) = rest.char_indices().take(12).find(|(_, c)| *c == ';') else {
        return not_entity;
    };
    let name = &rest[1..semi.0];
    let decoded = if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => number.parse().ok(),
        };
        code.map(|code| match char::from_u32(code) {
            Some(c) if code != 0 => c,
            _ => char::REPLACEMENT_CHARACTER,
        })
    } else {
        named_entity(name)
    };
    match decoded {
        Some(c) => (Some(c), semi.0 + 1),
        None => not_entity,
    }
}

fn named_entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        // Uno spazio come gli altri: gli spazi ripetuti si riducono a uno.
        "nbsp" => ' ',
        "ndash" => '–',
        "mdash" => '—',
        "hellip" => '…',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "laquo" => '«',
        "raquo" => '»',
        "bull" => '•',
        "middot" => '·',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "deg" => '°',
        "times" => '×',
        "euro" => '€',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::plain_text;

    #[test]
    fn paragraphs_breaks_and_lists() {
        let html = "<h2>Details!</h2>\n<p>A damage   meter.<br>For\n everyone.</p>\
                    <ul><li>Fast</li><li>Small</li></ul><p>End</p>";
        assert_eq!(
            plain_text(html),
            "Details!\n\nA damage meter.\nFor everyone.\n\nFast\nSmall\n\nEnd"
        );
        // Due `<br>` fanno una riga vuota, non di più; i blocchi vuoti non si sommano.
        assert_eq!(plain_text("a<br><br><br>b"), "a\n\nb");
        assert_eq!(plain_text("<div><p></p><div>a</div></div>"), "a");
        assert_eq!(
            plain_text("<table><tr><td>a</td><td>b</td></tr></table>"),
            "a b"
        );
    }

    #[test]
    fn entities_are_decoded() {
        assert_eq!(
            plain_text("Tom &amp; Jerry &lt;3 &quot;x&quot; &#39;y&#x27; caf&#233; a&nbsp;&nbsp;b"),
            "Tom & Jerry <3 \"x\" 'y' café a b"
        );
        assert_eq!(plain_text("&mdash; &hellip; &rsquo;"), "— … ’");
        // Quelle sconosciute o senza `;` restano come sono.
        assert_eq!(
            plain_text("&bogus; &amp &#xZZ; R&D"),
            "&bogus; &amp &#xZZ; R&D"
        );
        assert_eq!(plain_text("&#0;"), "\u{fffd}");
        // Un'entità decodificata è testo, mai un tag: non si interpreta come markup qui, e il
        // frontend non deve farlo (vedi il modulo).
        assert_eq!(
            plain_text("&lt;script&gt;x&lt;/script&gt;"),
            "<script>x</script>"
        );
    }

    #[test]
    fn scripts_styles_and_comments_are_dropped() {
        assert_eq!(
            plain_text(
                "<style>p { color: red }</style>a<SCRIPT type=\"x\">alert('<b>')</Script >b\
                 <!-- <p>hidden</p> -->c"
            ),
            "abc"
        );
        assert_eq!(plain_text("a<script>never closed"), "a");
        assert_eq!(plain_text("a<!-- never closed"), "a");
    }

    #[test]
    fn unclosed_tags_and_stray_brackets() {
        assert_eq!(plain_text("<p>one<p>two"), "one\n\ntwo");
        assert_eq!(plain_text("text <a href=\"x"), "text");
        assert_eq!(plain_text("1 < 2 and 3 > 2"), "1 < 2 and 3 > 2");
        assert_eq!(
            plain_text("<a title=\"a > b\" href='/x'>link</a> <img src=x>"),
            "link"
        );
        assert_eq!(plain_text("   \n "), "");
        assert_eq!(plain_text("ünïcödé <b>ok</b>"), "ünïcödé ok");
    }
}
