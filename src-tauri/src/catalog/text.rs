//! From the HTML of an addon's description to plain text.
//!
//! It is a defence: HTML written by an author, shown in the webview, would have access to
//! `invoke()`. Only text reaches the frontend: no tags, the content of `<script>` and
//! `<style>` discarded, the common entities decoded. Paragraphs and blocks become a blank
//! line, `<br>` and `<li>` a line break, repeated whitespace a single space. It is not a
//! complete HTML parser and does not need to be: what it does not recognise it discards or
//! leaves as text.
//!
//! **The result is text, not safe HTML.** Decoding the entities turns the markup characters
//! back into plain characters: `&lt;img src=x onerror=...&gt;` becomes the string
//! `<img src=x onerror=...>`. That is correct as long as it stays text, so the contract with
//! the frontend is that `description` is shown **only as text** (React's text binding,
//! `{description}`, or `textContent`), **never** with `innerHTML` or `dangerouslySetInnerHTML`.
//! `<`, `>` and `&` are not escaped here: the text binding already does it, and a double
//! escape would show `&lt;3` instead of `<3`.

/// The plain text of an HTML fragment. It must be shown as text, never as HTML (see the
/// module).
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

/// The produced text, with the spaces and line breaks still to be written: they are written
/// only before the next character, so they never remain at the start or at the end.
#[derive(Default)]
struct Writer {
    out: String,
    space: bool,
    /// 0 nothing, 1 a line break, 2 a blank line.
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

/// Processes the tag at the start of `rest` and returns what follows it.
fn tag<'a>(rest: &'a str, out: &mut Writer) -> &'a str {
    if let Some(comment) = rest.strip_prefix("<!--") {
        return comment.find("-->").map_or("", |end| &comment[end + 3..]);
    }
    // `a < b` is text: a tag starts with a letter, `/`, `!` or `?`.
    let opens_tag = rest[1..]
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?'));
    if !opens_tag {
        out.push('<');
        return &rest[1..];
    }
    // An unclosed tag at the end of the text is discarded along with what follows it.
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

/// The index of the `>` that closes the tag at the start of `rest`, not counting those inside
/// quotes in the attributes.
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

/// Skips the content of `<script>` or `<style>` up to and including the closing tag; without
/// a closing tag it discards all the rest.
fn skip_raw<'a>(rest: &'a str, name: &str) -> &'a str {
    // `to_ascii_lowercase` does not move bytes: the indices are valid for `rest` too.
    let lower = rest.to_ascii_lowercase();
    let Some(start) = lower.find(&format!("</{name}")) else {
        return "";
    };
    lower[start..]
        .find('>')
        .map_or("", |end| &rest[start + end + 1..])
}

/// The entity at the start of `rest` (which starts with `&`) and how many bytes it takes. An
/// entity it does not recognise counts as a literal `&`.
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
        // A space like any other: repeated spaces collapse into one.
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
        // Two `<br>` make a blank line, no more; empty blocks do not add up.
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
        // Unknown ones or those without `;` stay as they are.
        assert_eq!(
            plain_text("&bogus; &amp &#xZZ; R&D"),
            "&bogus; &amp &#xZZ; R&D"
        );
        assert_eq!(plain_text("&#0;"), "\u{fffd}");
        // A decoded entity is text, never a tag: it is not interpreted as markup here, and the
        // frontend must not do so either (see the module).
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
