//! Reading what a tool fetches or finds: a web page's text rather than its
//! markup, a PDF's text, a spreadsheet's cells, a document's and a deck's
//! words, and what a picture is.
//!
//! Everything here is Rust without `unsafe`, and bounded: what is read is
//! someone else's, and may be made to break a reader, so every stream and
//! every file of an archive inflates to a limit, and so does all of a
//! document. What cannot be read is said, never guessed: a PDF whose fonts
//! do not say which letters they draw, or one that is encrypted, is
//! refused with why.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

/// The most bytes one stream of a PDF, or one file of an archive, may
/// inflate to.
pub const MAX_INFLATED: usize = 32 << 20;

/// The most bytes all of a document's streams may inflate to.
pub const MAX_INFLATED_ALL: usize = 128 << 20;

/// The most pages of a PDF read.
pub const MAX_PAGES: usize = 2_000;

/// The most rows of a spreadsheet's sheet read.
pub const MAX_ROWS: usize = 10_000;

/// The most cells of a sheet kept.
pub const MAX_CELLS: usize = 1_000_000;

/// The widest row shown as a table, a tab for each column; a row with a
/// cell further right shows its cells named by their columns.
pub const MAX_COLUMNS_SHOWN: usize = 256;

/// What a file or a fetched page is, as far as reading it goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A web page.
    Html,
    /// A PDF.
    Pdf,
    /// An Excel workbook (`.xlsx`).
    Xlsx,
    /// A Word document (`.docx`).
    Docx,
    /// A PowerPoint deck (`.pptx`).
    Pptx,
    /// A picture, of this media type.
    Picture(&'static str),
    /// Text of another kind (plain text, JSON, CSV, code).
    Text,
    /// Bytes that are none of these.
    Binary,
}

impl Kind {
    /// What it is called, for what the model is told.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Html => "a web page",
            Kind::Pdf => "a PDF",
            Kind::Xlsx => "a spreadsheet",
            Kind::Docx => "a Word document",
            Kind::Pptx => "a slide deck",
            Kind::Picture(_) => "a picture",
            Kind::Text => "text",
            Kind::Binary => "binary data",
        }
    }

    /// Whether its text is read out of it here ([`text_of`]).
    pub fn is_document(self) -> bool {
        matches!(
            self,
            Kind::Html | Kind::Pdf | Kind::Xlsx | Kind::Docx | Kind::Pptx
        )
    }
}

/// What `bytes` are: by what they begin with first, then by `name` (a
/// file's name, or a URL) and the `content_type` they came with.
pub fn kind_of(bytes: &[u8], name: &str, content_type: Option<&str>) -> Kind {
    if let Some(media) = picture_type(bytes) {
        return Kind::Picture(media);
    }
    let head = &bytes[..bytes.len().min(1024)];
    if head.windows(5).any(|w| w == b"%PDF-") {
        return Kind::Pdf;
    }
    if bytes.starts_with(b"PK\x03\x04") {
        if let Ok(names) = zip_names(bytes) {
            let has = |n: &str| names.iter().any(|name| name == n);
            if has("word/document.xml") {
                return Kind::Docx;
            }
            if has("xl/workbook.xml") {
                return Kind::Xlsx;
            }
            if has("ppt/presentation.xml") {
                return Kind::Pptx;
            }
        }
        return Kind::Binary;
    }
    let media = content_type
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let path = name
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let text = match std::str::from_utf8(head) {
        Ok(text) => text,
        // A character cut at the end of the head is no sign of binary data.
        Err(e) if e.error_len().is_none() => {
            std::str::from_utf8(&head[..e.valid_up_to()]).unwrap_or_default()
        }
        Err(_) => return Kind::Binary,
    };
    let start = text
        .trim_start_matches('\u{feff}')
        .trim_start()
        .to_ascii_lowercase();
    if media == "text/html"
        || media == "application/xhtml+xml"
        || [".html", ".htm", ".xhtml"]
            .iter()
            .any(|x| path.ends_with(x))
        || start.starts_with("<!doctype html")
        || start.starts_with("<html")
    {
        return Kind::Html;
    }
    if text.contains('\0') {
        return Kind::Binary;
    }
    Kind::Text
}

/// The text of `bytes`, read as `kind` says, if it is a kind read here
/// ([`Kind::is_document`]): or why it could not be read. `base` is the
/// address a web page came from, for its links.
pub fn text_of(kind: Kind, bytes: &[u8], base: Option<&str>) -> Option<Result<String, String>> {
    Some(match kind {
        Kind::Html => Ok(html_to_text(&String::from_utf8_lossy(bytes), base)),
        Kind::Pdf => pdf_to_text(bytes),
        Kind::Xlsx => xlsx_to_text(bytes),
        Kind::Docx => docx_to_text(bytes),
        Kind::Pptx => pptx_to_text(bytes),
        _ => return None,
    })
}

/// `text` from its character `from`, at most `most` characters, with a note
/// of where it is in the whole and how to read on.
pub fn window(text: &str, from: usize, most: usize) -> String {
    let count = text.chars().count();
    if from >= count && count > 0 {
        return format!("[the text has {count} characters: there is nothing from {from}]");
    }
    let shown: String = text.chars().skip(from).take(most).collect();
    let to = from + shown.chars().count();
    let mut out = String::new();
    if from > 0 {
        out.push_str(&format!("[from character {from} of {count}]\n"));
    }
    out.push_str(&shown);
    if to < count {
        out.push_str(&format!(
            "\n[characters {from} to {to} of {count}: ask again with from {to} for the rest]"
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Pictures

/// The media type of a picture `bytes` hold, by what they begin with: PNG,
/// JPEG, GIF or WebP.
pub fn picture_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// How wide and high a picture is, in pixels, as its header says.
pub fn picture_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let be32 = |at: usize| -> Option<u32> {
        Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
    };
    let be16 = |at: usize| -> Option<u32> {
        Some(u32::from(u16::from_be_bytes(
            bytes.get(at..at + 2)?.try_into().ok()?,
        )))
    };
    let le16 = |at: usize| -> Option<u32> {
        Some(u32::from(u16::from_le_bytes(
            bytes.get(at..at + 2)?.try_into().ok()?,
        )))
    };
    let le24 = |at: usize| -> Option<u32> {
        let b = bytes.get(at..at + 3)?;
        Some(u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16)
    };
    match picture_type(bytes)? {
        "image/png" => Some((be32(16)?, be32(20)?)),
        "image/gif" => Some((le16(6)?, le16(8)?)),
        "image/webp" => match bytes.get(12..16)? {
            b"VP8X" => Some((le24(24)? + 1, le24(27)? + 1)),
            b"VP8 " => Some((le16(26)? & 0x3fff, le16(28)? & 0x3fff)),
            b"VP8L" => {
                let b = bytes.get(21..25)?;
                let bits = u32::from_le_bytes(b.try_into().ok()?);
                Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
            }
            _ => None,
        },
        _ => {
            // JPEG: the first start of frame.
            let mut at = 2;
            while at + 9 < bytes.len() {
                if bytes[at] != 0xff {
                    return None;
                }
                let marker = bytes[at + 1];
                if marker == 0xff {
                    at += 1;
                    continue;
                }
                let length = be16(at + 2)? as usize;
                let frame =
                    (0xc0..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker);
                if frame {
                    return Some((be16(at + 7)?, be16(at + 5)?));
                }
                at += 2 + length;
            }
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Markup: a lexer for HTML and XML

/// A piece of markup: text, or a tag.
enum Token<'a> {
    Text(&'a str),
    Tag(Tag<'a>),
}

/// A tag: its name (lowercased), whether it closes, whether it closes
/// itself, and its attributes as written.
struct Tag<'a> {
    name: String,
    closing: bool,
    self_closing: bool,
    attrs: &'a str,
}

impl Tag<'_> {
    /// The value of its attribute `key` (lowercased), entities decoded.
    fn attr(&self, key: &str) -> Option<String> {
        let mut rest = self.attrs;
        loop {
            rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
            if rest.is_empty() {
                return None;
            }
            let end = rest
                .find(|c: char| c.is_whitespace() || c == '=' || c == '/' || c == '>')
                .unwrap_or(rest.len());
            let name = rest[..end].to_ascii_lowercase();
            rest = rest[end..].trim_start();
            let value = if let Some(after) = rest.strip_prefix('=') {
                let after = after.trim_start();
                let (value, left) = match after.chars().next() {
                    Some(q @ ('"' | '\'')) => {
                        let inner = &after[1..];
                        let close = inner.find(q).unwrap_or(inner.len());
                        (&inner[..close], &inner[(close + 1).min(inner.len())..])
                    }
                    _ => {
                        let close = after.find(char::is_whitespace).unwrap_or(after.len());
                        (&after[..close], &after[close..])
                    }
                };
                rest = left;
                value
            } else {
                ""
            };
            if name == key || name.rsplit(':').next() == Some(key) {
                return Some(decode_entities(value).into_owned());
            }
        }
    }
}

/// Markup, read a token at a time.
struct Lexer<'a> {
    src: &'a str,
    at: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Lexer<'a> {
        Lexer { src, at: 0 }
    }

    /// Goes past the end of the element `name`, whose content is not
    /// markup (a script, a style): returns that content.
    fn raw(&mut self, name: &str) -> &'a str {
        let rest = &self.src[self.at..];
        let lower = rest.to_ascii_lowercase();
        let close = format!("</{name}");
        match lower.find(&close) {
            Some(i) => {
                let content = &rest[..i];
                let after = &rest[i..];
                let end = after.find('>').map_or(after.len(), |j| j + 1);
                self.at += i + end;
                content
            }
            None => {
                self.at = self.src.len();
                rest
            }
        }
    }
}

impl<'a> Iterator for Lexer<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        loop {
            let rest = &self.src[self.at..];
            if rest.is_empty() {
                return None;
            }
            if !rest.starts_with('<') {
                let end = rest.find('<').unwrap_or(rest.len());
                self.at += end;
                return Some(Token::Text(&rest[..end]));
            }
            if let Some(after) = rest.strip_prefix("<!--") {
                let end = after.find("-->").map_or(after.len(), |i| i + 3);
                self.at += 4 + end;
                continue;
            }
            if let Some(after) = rest.strip_prefix("<![CDATA[") {
                let end = after.find("]]>").unwrap_or(after.len());
                self.at += 9 + (end + 3).min(after.len());
                return Some(Token::Text(&after[..end]));
            }
            if rest.starts_with("<!") || rest.starts_with("<?") {
                let end = rest.find('>').map_or(rest.len(), |i| i + 1);
                self.at += end;
                continue;
            }
            let closing = rest.starts_with("</");
            let start = if closing { 2 } else { 1 };
            let starts_name = rest[start..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic());
            if !starts_name {
                // A lone `<` is text.
                self.at += 1;
                return Some(Token::Text(&rest[..1]));
            }
            // The tag's end: the first `>` outside quotes.
            let bytes = rest.as_bytes();
            let mut i = start;
            let mut quote = None;
            while i < bytes.len() {
                match (quote, bytes[i]) {
                    (None, b'"') | (None, b'\'') => quote = Some(bytes[i]),
                    (Some(q), b) if b == q => quote = None,
                    (None, b'>') => break,
                    _ => {}
                }
                i += 1;
            }
            let inside = &rest[start..i.min(rest.len())];
            self.at += (i + 1).min(rest.len());
            let name_end = inside
                .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
                .unwrap_or(inside.len());
            let name = inside[..name_end].to_ascii_lowercase();
            let attrs = &inside[name_end..];
            let self_closing = attrs.trim_end().ends_with('/');
            return Some(Token::Tag(Tag {
                name,
                closing,
                self_closing,
                attrs,
            }));
        }
    }
}

/// `text` with its character references decoded: `&amp;`, `&#233;`,
/// `&#xe9;` and the common named ones.
pub fn decode_entities(text: &str) -> Cow<'_, str> {
    if !text.contains('&') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let end = after
            .char_indices()
            .take(32)
            .find(|(_, c)| *c == ';')
            .map(|(j, _)| j);
        let decoded = end.and_then(|j| {
            let name = &after[..j];
            let c = if let Some(number) = name.strip_prefix('#') {
                let code = match number.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => number.parse::<u32>().ok(),
                }?;
                char::from_u32(code).map(|c| c.to_string())
            } else {
                named_entity(name).map(str::to_string)
            }?;
            Some((c, j + 1))
        });
        match decoded {
            Some((c, used)) => {
                out.push_str(&c);
                rest = &after[used..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

fn named_entity(name: &str) -> Option<&'static str> {
    Some(match name {
        "amp" | "AMP" => "&",
        "lt" | "LT" => "<",
        "gt" | "GT" => ">",
        "quot" | "QUOT" => "\"",
        "apos" => "'",
        "nbsp" => "\u{a0}",
        "ndash" => "–",
        "mdash" => "—",
        "hellip" => "…",
        "lsquo" => "‘",
        "rsquo" => "’",
        "sbquo" => "‚",
        "ldquo" => "“",
        "rdquo" => "”",
        "bdquo" => "„",
        "laquo" => "«",
        "raquo" => "»",
        "lsaquo" => "‹",
        "rsaquo" => "›",
        "copy" => "©",
        "reg" => "®",
        "trade" => "™",
        "deg" => "°",
        "plusmn" => "±",
        "times" => "×",
        "divide" => "÷",
        "middot" => "·",
        "bull" => "•",
        "euro" => "€",
        "pound" => "£",
        "yen" => "¥",
        "cent" => "¢",
        "sect" => "§",
        "para" => "¶",
        "frac12" => "½",
        "frac14" => "¼",
        "frac34" => "¾",
        "sup2" => "²",
        "sup3" => "³",
        "micro" => "µ",
        "minus" => "−",
        "larr" => "←",
        "rarr" => "→",
        "uarr" => "↑",
        "darr" => "↓",
        "le" => "≤",
        "ge" => "≥",
        "ne" => "≠",
        "asymp" => "≈",
        "infin" => "∞",
        "shy" => "\u{ad}",
        "zwj" => "\u{200d}",
        "zwnj" => "\u{200c}",
        "thinsp" => "\u{2009}",
        "ensp" => "\u{2002}",
        "emsp" => "\u{2003}",
        "iexcl" => "¡",
        "iquest" => "¿",
        "aacute" => "á",
        "agrave" => "à",
        "acirc" => "â",
        "auml" => "ä",
        "atilde" => "ã",
        "aring" => "å",
        "aelig" => "æ",
        "ccedil" => "ç",
        "eacute" => "é",
        "egrave" => "è",
        "ecirc" => "ê",
        "euml" => "ë",
        "iacute" => "í",
        "igrave" => "ì",
        "icirc" => "î",
        "iuml" => "ï",
        "ntilde" => "ñ",
        "oacute" => "ó",
        "ograve" => "ò",
        "ocirc" => "ô",
        "ouml" => "ö",
        "otilde" => "õ",
        "oslash" => "ø",
        "uacute" => "ú",
        "ugrave" => "ù",
        "ucirc" => "û",
        "uuml" => "ü",
        "yacute" => "ý",
        "yuml" => "ÿ",
        "szlig" => "ß",
        "Aacute" => "Á",
        "Agrave" => "À",
        "Auml" => "Ä",
        "Ccedil" => "Ç",
        "Eacute" => "É",
        "Egrave" => "È",
        "Ntilde" => "Ñ",
        "Oacute" => "Ó",
        "Ouml" => "Ö",
        "Uacute" => "Ú",
        "Uuml" => "Ü",
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// HTML to text

/// Elements whose content is not markup, and not read.
const RAW: [&str; 3] = ["script", "style", "textarea"];

/// Elements left out with all they hold: what draws, what runs, and a
/// page's furniture (its menus, its footer, its forms).
const LEFT_OUT: [&str; 12] = [
    "noscript", "template", "svg", "iframe", "object", "canvas", "select", "button", "nav",
    "aside", "footer", "form",
];

/// Text being written out: words with single spaces between them, lines
/// and paragraphs.
#[derive(Default)]
struct Out {
    text: String,
    space: bool,
}

impl Out {
    fn at_line_start(&self) -> bool {
        self.text.is_empty() || self.text.ends_with('\n')
    }

    /// Words, their runs of white space made one space.
    fn words(&mut self, text: &str) {
        for c in text.chars() {
            if c.is_whitespace() {
                self.space = true;
                continue;
            }
            if self.space && !self.at_line_start() {
                self.text.push(' ');
            }
            self.space = false;
            self.text.push(c);
        }
    }

    /// Text as it is (in `<pre>`).
    fn raw(&mut self, text: &str) {
        if self.space && !self.at_line_start() {
            self.text.push(' ');
        }
        self.space = false;
        self.text.push_str(text);
    }

    /// A mark that opens (`[`, `` ` ``): after the space before it.
    fn open(&mut self, mark: &str) {
        if self.space && !self.at_line_start() {
            self.text.push(' ');
        }
        self.space = false;
        self.text.push_str(mark);
    }

    /// A mark that closes: before the space after it.
    fn close(&mut self, mark: &str) {
        self.text.push_str(mark);
    }

    fn newline(&mut self) {
        while self.text.ends_with(' ') {
            self.text.pop();
        }
        if !self.at_line_start() {
            self.text.push('\n');
        }
        self.space = false;
    }

    fn paragraph(&mut self) {
        self.newline();
        if !self.text.is_empty() && !self.text.ends_with("\n\n") {
            self.text.push('\n');
        }
    }

    fn finish(self) -> String {
        let mut out = String::with_capacity(self.text.len());
        let mut blank = 0;
        for line in self.text.lines() {
            let line = line.trim_end();
            if line.trim().is_empty() {
                blank += 1;
                continue;
            }
            if !out.is_empty() {
                out.push_str(if blank > 0 { "\n\n" } else { "\n" });
            }
            blank = 0;
            out.push_str(line);
        }
        out
    }
}

/// A web page's text: its title first, headings marked `#`, list items
/// `-` (or numbered), table cells between `|`, links as `[text](address)`
/// with their addresses made whole from `base`, pictures by what they say
/// they show; scripts, styles, menus, footers and forms left out.
pub fn html_to_text(html: &str, base: Option<&str>) -> String {
    let mut out = Out::default();
    let mut base = base.map(str::to_string);
    let mut title = String::new();
    let mut skipping: Vec<String> = Vec::new();
    let mut pre = 0usize;
    let mut lists: Vec<Option<u32>> = Vec::new();
    let mut links: Vec<Option<(usize, String)>> = Vec::new();
    let mut cells = 0usize;
    let mut lexer = Lexer::new(html);
    while let Some(token) = lexer.next() {
        let tag = match token {
            Token::Text(text) => {
                if skipping.is_empty() {
                    let text = decode_entities(text);
                    if pre > 0 {
                        out.raw(&text);
                    } else {
                        out.words(&text);
                    }
                }
                continue;
            }
            Token::Tag(tag) => tag,
        };
        let name = tag.name.as_str();
        if let Some(open) = skipping.last() {
            if name == open {
                if tag.closing {
                    skipping.pop();
                } else if !tag.self_closing {
                    skipping.push(name.to_string());
                }
            }
            continue;
        }
        if tag.closing && name != "title" && RAW.contains(&name) {
            continue;
        }
        if !tag.closing && RAW.contains(&name) {
            lexer.raw(name);
            continue;
        }
        if name == "title" && !tag.closing {
            if title.is_empty() {
                let mut words = Out::default();
                words.words(&decode_entities(lexer.raw("title")));
                title = words.finish();
            } else {
                lexer.raw("title");
            }
            continue;
        }
        if !tag.closing && !tag.self_closing && LEFT_OUT.contains(&name) {
            skipping.push(name.to_string());
            continue;
        }
        let heading = name
            .strip_prefix('h')
            .and_then(|n| n.parse::<usize>().ok())
            .filter(|n| (1..=6).contains(n));
        match (name, tag.closing) {
            ("base", false) => {
                if let Some(href) = tag.attr("href") {
                    base = resolve(base.as_deref(), &href).or(Some(href));
                }
            }
            ("br", _) => out.newline(),
            ("hr", false) => {
                out.paragraph();
                out.raw("---");
                out.paragraph();
            }
            (_, false) if heading.is_some() => {
                out.paragraph();
                out.raw(&"#".repeat(heading.unwrap_or(1)));
                out.raw(" ");
            }
            (_, true) if heading.is_some() => out.paragraph(),
            ("pre", false) => {
                out.paragraph();
                pre += 1;
            }
            ("pre", true) => {
                pre = pre.saturating_sub(1);
                out.paragraph();
            }
            ("ul", false) => {
                out.newline();
                lists.push(None);
            }
            ("ol", false) => {
                out.newline();
                let start = tag.attr("start").and_then(|s| s.parse().ok()).unwrap_or(1);
                lists.push(Some(start));
            }
            ("ul" | "ol", true) => {
                lists.pop();
                out.newline();
                if lists.is_empty() {
                    out.paragraph();
                }
            }
            ("li", false) => {
                out.newline();
                let depth = lists.len().saturating_sub(1);
                let marker = match lists.last_mut() {
                    Some(Some(n)) => {
                        let marker = format!("{n}. ");
                        *n += 1;
                        marker
                    }
                    _ => "- ".to_string(),
                };
                out.raw(&format!("{}{marker}", "  ".repeat(depth)));
            }
            ("tr", _) => {
                out.newline();
                cells = 0;
            }
            ("td" | "th", false) => {
                if cells > 0 {
                    out.close(" | ");
                }
                cells += 1;
            }
            ("table", _) | ("p", _) | ("blockquote", _) | ("figure", _) | ("dl", _) => {
                out.paragraph()
            }
            ("div" | "section" | "article" | "main" | "header" | "address" | "dt" | "dd", _)
            | ("figcaption" | "summary" | "caption" | "details" | "li" | "tbody", _) => {
                out.newline()
            }
            ("a", false) => {
                let target = tag
                    .attr("href")
                    .and_then(|href| link_target(base.as_deref(), &href));
                match target {
                    Some(target) => {
                        out.open("[");
                        links.push(Some((out.text.len(), target)));
                    }
                    None => links.push(None),
                }
            }
            ("a", true) => {
                if let Some(Some((at, target))) = links.pop() {
                    let said = out.text[at..].trim().to_string();
                    if said.is_empty() {
                        out.text.truncate(at.saturating_sub(1));
                        while out.text.ends_with(' ') {
                            out.text.pop();
                        }
                        out.space = true;
                    } else if said == target {
                        // The address is the text: once is enough.
                        out.text.remove(at - 1);
                    } else {
                        out.close(&format!("]({target})"));
                    }
                }
            }
            ("img", _) => {
                if let Some(alt) = tag.attr("alt").filter(|a| !a.trim().is_empty()) {
                    out.open("");
                    out.words(&format!("[image: {}]", alt.trim()));
                    out.space = true;
                }
            }
            ("code", false) if pre == 0 => out.open("`"),
            ("code", true) if pre == 0 => out.close("`"),
            _ => {}
        }
    }
    let body = out.finish();
    match title.trim() {
        "" => body,
        title if body.starts_with(title) || body.starts_with(&format!("# {title}")) => body,
        title => format!("{title}\n\n{body}"),
    }
}

/// Where a link leads, made whole from `base`: none for a link within the
/// page or one that runs a script.
fn link_target(base: Option<&str>, href: &str) -> Option<String> {
    let href = href.trim();
    let lower = href.to_ascii_lowercase();
    if href.is_empty() || href.starts_with('#') || lower.starts_with("javascript:") {
        return None;
    }
    if lower.starts_with("data:") {
        return None;
    }
    Some(resolve(base, href).unwrap_or_else(|| href.to_string()))
}

/// `href` made whole from `base` (an absolute address): itself if it is
/// whole already, none if `base` is not an address.
pub fn resolve(base: Option<&str>, href: &str) -> Option<String> {
    let lower = href.to_ascii_lowercase();
    if lower.contains("://") || lower.starts_with("mailto:") || lower.starts_with("tel:") {
        return Some(href.to_string());
    }
    let (scheme, rest) = base?.split_once("://")?;
    if let Some(net) = href.strip_prefix("//") {
        return Some(format!("{scheme}://{net}"));
    }
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let origin = format!("{scheme}://{}", &rest[..host_end]);
    let path = rest[host_end..].split(['?', '#']).next().unwrap_or("");
    if href.starts_with('?') {
        let path = if path.is_empty() { "/" } else { path };
        return Some(format!("{origin}{path}{href}"));
    }
    let joined = if href.starts_with('/') {
        href.to_string()
    } else {
        let dir = match path.rfind('/') {
            Some(i) => &path[..=i],
            None => "/",
        };
        format!("{dir}{href}")
    };
    // `.` and `..` resolved, in the path only.
    let (path, tail) = match joined.find(['?', '#']) {
        Some(i) => (&joined[..i], &joined[i..]),
        None => (joined.as_str(), ""),
    };
    let mut parts: Vec<&str> = Vec::new();
    let segments: Vec<&str> = path.split('/').collect();
    for (i, part) in segments.iter().enumerate() {
        match *part {
            "." => {}
            ".." => {
                parts.pop();
            }
            "" if i > 0 && i + 1 < segments.len() => {}
            part => parts.push(part),
        }
    }
    let mut path = parts.join("/");
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    if (joined.ends_with("/.") || joined.ends_with("/..")) && !path.ends_with('/') {
        path.push('/');
    }
    Some(format!("{origin}{path}{tail}"))
}

// ---------------------------------------------------------------------------
// ZIP archives (the Office formats)

fn le16(bytes: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?) as usize)
}

fn le32(bytes: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize)
}

/// A file of a ZIP archive, as its central directory has it.
struct Entry {
    name: String,
    method: usize,
    compressed: usize,
    local: usize,
}

/// An archive's central directory: each file's name, how it is compressed,
/// its size compressed and where its local header is.
fn zip_directory(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    let from = bytes.len().saturating_sub(22 + 65_535);
    let end = (from..bytes.len().saturating_sub(21))
        .rev()
        .find(|&i| bytes[i..].starts_with(b"PK\x05\x06"))
        .ok_or("not a ZIP archive: it has no end of its directory")?;
    let count = le16(bytes, end + 10).ok_or("a cut ZIP archive")?;
    let offset = le32(bytes, end + 16).ok_or("a cut ZIP archive")?;
    if offset == 0xffff_ffff || count == 0xffff {
        return Err("a ZIP64 archive, which is not read here".into());
    }
    let mut entries = Vec::new();
    let mut at = offset;
    for _ in 0..count {
        if !bytes
            .get(at..)
            .is_some_and(|b| b.starts_with(b"PK\x01\x02"))
        {
            return Err("a damaged ZIP archive: its directory is not where it says".into());
        }
        let flags = le16(bytes, at + 8).ok_or("a cut ZIP archive")?;
        let method = le16(bytes, at + 10).ok_or("a cut ZIP archive")?;
        let compressed = le32(bytes, at + 20).ok_or("a cut ZIP archive")?;
        let name_len = le16(bytes, at + 28).ok_or("a cut ZIP archive")?;
        let extra = le16(bytes, at + 30).ok_or("a cut ZIP archive")?;
        let comment = le16(bytes, at + 32).ok_or("a cut ZIP archive")?;
        let local = le32(bytes, at + 42).ok_or("a cut ZIP archive")?;
        let name = bytes
            .get(at + 46..at + 46 + name_len)
            .ok_or("a cut ZIP archive")?;
        if flags & 1 == 0 {
            entries.push(Entry {
                name: String::from_utf8_lossy(name).into_owned(),
                method,
                compressed,
                local,
            });
        }
        at += 46 + name_len + extra + comment;
    }
    Ok(entries)
}

/// The names of the files of a ZIP archive (encrypted ones left out).
pub fn zip_names(bytes: &[u8]) -> Result<Vec<String>, String> {
    Ok(zip_directory(bytes)?.into_iter().map(|e| e.name).collect())
}

/// The files of a ZIP archive whose names `wanted` takes, inflated, each to
/// at most [`MAX_INFLATED`] bytes and all to [`MAX_INFLATED_ALL`].
fn zip_read(
    bytes: &[u8],
    wanted: impl Fn(&str) -> bool,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut files = BTreeMap::new();
    let mut left = MAX_INFLATED_ALL;
    for Entry {
        name,
        method,
        compressed,
        local,
    } in zip_directory(bytes)?
    {
        if !wanted(&name) {
            continue;
        }
        if !bytes
            .get(local..)
            .is_some_and(|b| b.starts_with(b"PK\x03\x04"))
        {
            return Err(format!(
                "{name}: its header is not where the directory says"
            ));
        }
        let name_len = le16(bytes, local + 26).ok_or("a cut ZIP archive")?;
        let extra = le16(bytes, local + 28).ok_or("a cut ZIP archive")?;
        let start = local + 30 + name_len + extra;
        let data = bytes
            .get(start..start + compressed)
            .ok_or_else(|| format!("{name} is cut short"))?;
        let limit = MAX_INFLATED.min(left);
        let inflated = match method {
            0 if data.len() <= limit => data.to_vec(),
            0 => return Err(format!("{name} is larger than {limit} bytes")),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(data, limit)
                .map_err(|e| format!("{name} does not inflate: {e:?}"))?,
            other => {
                return Err(format!(
                    "{name} is compressed in a way not read here ({other})"
                ))
            }
        };
        left = left.saturating_sub(inflated.len());
        files.insert(name, inflated);
    }
    Ok(files)
}

/// An XML file of an archive, as text.
fn xml_of<'a>(files: &'a BTreeMap<String, Vec<u8>>, name: &str) -> Option<Cow<'a, str>> {
    files.get(name).map(|bytes| String::from_utf8_lossy(bytes))
}

// ---------------------------------------------------------------------------
// Spreadsheets, documents and decks

/// A column's number (from 1) from a cell reference (`B7` → 2).
fn column_of(reference: &str) -> usize {
    reference
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .fold(0usize, |n, c| {
            n.saturating_mul(26)
                .saturating_add(c.to_ascii_uppercase() as usize - 'A' as usize + 1)
        })
}

/// A column's letters from its number (2 → `B`).
fn column_name(mut n: usize) -> String {
    let mut name = Vec::new();
    while n > 0 {
        n -= 1;
        name.push((b'A' + (n % 26) as u8) as char);
        n /= 26;
    }
    name.iter().rev().collect()
}

/// An Excel workbook's sheets, in order: each its name, then its rows,
/// each its number and its cells' values between tabs, under a line that
/// names the columns.
pub fn xlsx_to_text(bytes: &[u8]) -> Result<String, String> {
    let files = zip_read(bytes, |name| {
        name == "xl/workbook.xml"
            || name == "xl/_rels/workbook.xml.rels"
            || name == "xl/sharedStrings.xml"
            || (name.starts_with("xl/worksheets/") && name.ends_with(".xml"))
    })?;
    let workbook =
        xml_of(&files, "xl/workbook.xml").ok_or("the workbook has no xl/workbook.xml")?;
    // The strings the cells share.
    let mut shared: Vec<String> = Vec::new();
    if let Some(xml) = xml_of(&files, "xl/sharedStrings.xml") {
        let mut current: Option<String> = None;
        let mut in_text = false;
        let mut phonetic = false;
        for token in Lexer::new(&xml) {
            match token {
                Token::Tag(tag) => match (local(&tag.name), tag.closing) {
                    ("si", false) => current = Some(String::new()),
                    ("si", true) => shared.push(current.take().unwrap_or_default()),
                    ("rph", false) => phonetic = !tag.self_closing,
                    ("rph", true) => phonetic = false,
                    ("t", false) => in_text = !tag.self_closing,
                    ("t", true) => in_text = false,
                    _ => {}
                },
                Token::Text(text) if in_text && !phonetic => {
                    if let Some(current) = &mut current {
                        current.push_str(&decode_entities(text));
                    }
                }
                Token::Text(_) => {}
            }
        }
    }
    // Which file each sheet is.
    let mut targets: BTreeMap<String, String> = BTreeMap::new();
    if let Some(rels) = xml_of(&files, "xl/_rels/workbook.xml.rels") {
        for token in Lexer::new(&rels) {
            if let Token::Tag(tag) = token {
                if local(&tag.name) == "relationship" {
                    if let (Some(id), Some(target)) = (tag.attr("id"), tag.attr("target")) {
                        let target = match target.strip_prefix('/') {
                            Some(absolute) => absolute.to_string(),
                            None => format!("xl/{target}"),
                        };
                        targets.insert(id, target);
                    }
                }
            }
        }
    }
    let mut sheets: Vec<(String, String)> = Vec::new();
    for token in Lexer::new(&workbook) {
        if let Token::Tag(tag) = token {
            if local(&tag.name) == "sheet" && !tag.closing {
                let name = tag.attr("name").unwrap_or_else(|| "?".into());
                let file = tag
                    .attr("id")
                    .and_then(|id| targets.get(&id).cloned())
                    .unwrap_or_else(|| format!("xl/worksheets/sheet{}.xml", sheets.len() + 1));
                sheets.push((name, file));
            }
        }
    }
    let mut out = Vec::new();
    for (name, file) in sheets {
        let Some(xml) = xml_of(&files, &file) else {
            out.push(format!(
                "## Sheet: {name}\n(its cells are not in the workbook)"
            ));
            continue;
        };
        let rows = sheet_rows(&xml, &shared);
        // The header's columns: as far as the widest row shown as a table.
        let widest = rows
            .values()
            .filter_map(|r| r.keys().next_back().copied())
            .filter(|last| *last <= MAX_COLUMNS_SHOWN)
            .max()
            .unwrap_or(0);
        let counted = match rows.len() {
            1 => "1 row".to_string(),
            n => format!("{n} rows"),
        };
        let mut lines = vec![format!("## Sheet: {name} ({counted})")];
        if widest > 0 {
            let header: Vec<String> = (1..=widest).map(column_name).collect();
            lines.push(format!("row\t{}", header.join("\t")));
        }
        for (number, cells) in rows.iter().take(MAX_ROWS) {
            let last = cells.keys().next_back().copied().unwrap_or(0);
            let line = if last <= MAX_COLUMNS_SHOWN {
                // As a table: a tab for each column, empty ones too.
                (1..=last)
                    .map(|c| cells.get(&c).map(String::as_str).unwrap_or(""))
                    .collect::<Vec<_>>()
                    .join("\t")
            } else {
                // Cells far apart: each named by its column.
                cells
                    .iter()
                    .map(|(c, v)| format!("{}: {v}", column_name(*c)))
                    .collect::<Vec<_>>()
                    .join("\t")
            };
            lines.push(format!("{number}\t{}", line.trim_end()));
        }
        if rows.len() > MAX_ROWS {
            lines.push(format!("[… {} rows more]", rows.len() - MAX_ROWS));
        }
        out.push(lines.join("\n"));
    }
    Ok(out.join("\n\n"))
}

/// A sheet's rows by their numbers, each its cells' values by column.
fn sheet_rows(xml: &str, shared: &[String]) -> BTreeMap<usize, BTreeMap<usize, String>> {
    let mut rows: BTreeMap<usize, BTreeMap<usize, String>> = BTreeMap::new();
    let mut kept = 0usize;
    let mut row = 0usize;
    let mut column = 0usize;
    let mut kind = String::new();
    let mut value = String::new();
    let mut formula = String::new();
    let mut reading = false;
    let mut in_formula = false;
    let mut in_cell = false;
    for token in Lexer::new(xml) {
        match token {
            Token::Tag(tag) => match (local(&tag.name), tag.closing) {
                ("row", false) => {
                    row = tag
                        .attr("r")
                        .and_then(|r| r.parse().ok())
                        .unwrap_or(row + 1);
                    column = 0;
                }
                ("c", false) => {
                    column = tag
                        .attr("r")
                        .map(|r| column_of(&r))
                        .filter(|c| *c > 0)
                        .unwrap_or(column + 1);
                    kind = tag.attr("t").unwrap_or_default();
                    value.clear();
                    formula.clear();
                    in_cell = !tag.self_closing;
                }
                ("v" | "t", false) if in_cell => reading = !tag.self_closing,
                ("v" | "t", true) => reading = false,
                ("f", false) if in_cell => in_formula = !tag.self_closing,
                ("f", true) => in_formula = false,
                ("c", true) => {
                    in_cell = false;
                    let shown = match kind.as_str() {
                        "s" => value
                            .trim()
                            .parse::<usize>()
                            .ok()
                            .and_then(|i| shared.get(i).cloned())
                            .unwrap_or_default(),
                        "b" => match value.trim() {
                            "1" => "TRUE".into(),
                            _ => "FALSE".into(),
                        },
                        // A formula never worked out: the formula.
                        _ if value.is_empty() && !formula.is_empty() => format!("={formula}"),
                        _ => value.clone(),
                    };
                    let shown = shown.replace(['\t', '\n', '\r'], " ");
                    if !shown.is_empty()
                        && row > 0
                        && column > 0
                        && column <= 16_384
                        && kept < MAX_CELLS
                    {
                        kept += 1;
                        rows.entry(row).or_default().insert(column, shown);
                    }
                }
                _ => {}
            },
            Token::Text(text) if reading => value.push_str(&decode_entities(text)),
            Token::Text(text) if in_formula => formula.push_str(&decode_entities(text)),
            Token::Text(_) => {}
        }
    }
    rows
}

/// A tag's name without its prefix (`w:t` → `t`).
fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// A Word document's text: its paragraphs a line each (headings marked
/// `#`, list items `-`), its tables' cells between `|`.
pub fn docx_to_text(bytes: &[u8]) -> Result<String, String> {
    let files = zip_read(bytes, |name| name == "word/document.xml")?;
    let xml = xml_of(&files, "word/document.xml").ok_or("the document has no word/document.xml")?;
    let mut out = String::new();
    let mut paragraph = String::new();
    let mut prefix = String::new();
    let mut reading = false;
    let mut cell = 0usize;
    let mut in_row = false;
    for token in Lexer::new(&xml) {
        match token {
            Token::Tag(tag) => match (tag.name.as_str(), tag.closing) {
                ("w:p", false) => {
                    paragraph.clear();
                    prefix.clear();
                }
                ("w:pstyle", _) => {
                    let style = tag.attr("val").unwrap_or_default().to_ascii_lowercase();
                    if let Some(level) = style
                        .strip_prefix("heading")
                        .and_then(|n| n.trim().parse::<usize>().ok())
                    {
                        prefix = format!("{} ", "#".repeat(level.clamp(1, 6)));
                    } else if style == "title" {
                        prefix = "# ".into();
                    } else if style.contains("list") {
                        prefix = "- ".into();
                    }
                }
                ("w:numpr", false) if prefix.is_empty() => prefix = "- ".into(),
                ("w:t", false) => reading = !tag.self_closing,
                ("w:t", true) => reading = false,
                ("w:tab", _) => paragraph.push('\t'),
                ("w:br" | "w:cr", _) => paragraph.push('\n'),
                ("w:tr", false) => {
                    in_row = true;
                    cell = 0;
                }
                ("w:tr", true) => {
                    in_row = false;
                    out.push('\n');
                }
                ("w:tc", false) => {
                    if cell > 0 {
                        out.push_str(" | ");
                    }
                    cell += 1;
                }
                ("w:p", true) => {
                    let text = paragraph.trim();
                    if in_row {
                        if !text.is_empty() {
                            if !out.ends_with(" | ") && !out.ends_with('\n') && !out.is_empty() {
                                out.push(' ');
                            }
                            out.push_str(text);
                        }
                    } else if !text.is_empty() {
                        out.push_str(&prefix);
                        out.push_str(text);
                        out.push('\n');
                    } else {
                        out.push('\n');
                    }
                    paragraph.clear();
                }
                _ => {}
            },
            Token::Text(text) if reading => paragraph.push_str(&decode_entities(text)),
            Token::Text(_) => {}
        }
    }
    let mut cleaned = Out::default();
    cleaned.raw(&out);
    Ok(cleaned.finish())
}

/// A deck's slides, in order: each its number, then its text, a paragraph
/// a line.
pub fn pptx_to_text(bytes: &[u8]) -> Result<String, String> {
    let slide_number = |name: &str| -> Option<usize> {
        name.strip_prefix("ppt/slides/slide")?
            .strip_suffix(".xml")?
            .parse()
            .ok()
    };
    let files = zip_read(bytes, |name| slide_number(name).is_some())?;
    let mut slides: Vec<(usize, &Vec<u8>)> = files
        .iter()
        .filter_map(|(name, bytes)| Some((slide_number(name)?, bytes)))
        .collect();
    slides.sort_by_key(|(n, _)| *n);
    let mut out = Vec::new();
    for (n, bytes) in slides {
        let xml = String::from_utf8_lossy(bytes);
        let mut lines = vec![format!("## Slide {n}")];
        let mut paragraph = String::new();
        let mut reading = false;
        for token in Lexer::new(&xml) {
            match token {
                Token::Tag(tag) => match (tag.name.as_str(), tag.closing) {
                    ("a:t", false) => reading = !tag.self_closing,
                    ("a:t", true) => reading = false,
                    ("a:br", _) => paragraph.push('\n'),
                    ("a:p", true) => {
                        if !paragraph.trim().is_empty() {
                            lines.push(paragraph.trim().to_string());
                        }
                        paragraph.clear();
                    }
                    _ => {}
                },
                Token::Text(text) if reading => paragraph.push_str(&decode_entities(text)),
                Token::Text(_) => {}
            }
        }
        out.push(lines.join("\n"));
    }
    if out.is_empty() {
        return Err("the deck has no slides".into());
    }
    Ok(out.join("\n\n"))
}

// ---------------------------------------------------------------------------
// PDF

/// Nothing, where a value is wanted and none is there.
static NULL: Pdf = Pdf::Null;

/// A value of a PDF object.
#[derive(Clone, Debug, PartialEq)]
enum Pdf {
    Null,
    Bool(bool),
    Num(f64),
    Name(String),
    Str(Vec<u8>),
    Array(Vec<Pdf>),
    Dict(BTreeMap<String, Pdf>),
    Ref(u32),
    /// An operator, in a content stream.
    Op(String),
}

impl Pdf {
    fn get(&self, key: &str) -> Option<&Pdf> {
        match self {
            Pdf::Dict(map) => map.get(key),
            _ => None,
        }
    }

    fn name(&self) -> Option<&str> {
        match self {
            Pdf::Name(name) => Some(name),
            _ => None,
        }
    }

    fn num(&self) -> Option<f64> {
        match self {
            Pdf::Num(n) => Some(*n),
            _ => None,
        }
    }
}

fn is_white(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b'\x0c' | b'\0')
}

fn is_delimiter(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// Reads PDF values from bytes.
struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Parser<'a> {
    fn new(bytes: &'a [u8]) -> Parser<'a> {
        Parser { bytes, at: 0 }
    }

    fn skip_white(&mut self) {
        while self.at < self.bytes.len() {
            let b = self.bytes[self.at];
            if is_white(b) {
                self.at += 1;
            } else if b == b'%' {
                while self.at < self.bytes.len() && !matches!(self.bytes[self.at], b'\r' | b'\n') {
                    self.at += 1;
                }
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    /// The next value, or `None` at the end; an array or a dictionary is
    /// read whole, `depth` deep at most.
    fn value(&mut self, depth: usize) -> Option<Pdf> {
        self.skip_white();
        let b = self.peek()?;
        match b {
            b'/' => {
                self.at += 1;
                Some(Pdf::Name(self.name()))
            }
            b'(' => {
                self.at += 1;
                Some(Pdf::Str(self.literal()))
            }
            b'<' if self.bytes.get(self.at + 1) == Some(&b'<') => {
                self.at += 2;
                let mut map = BTreeMap::new();
                loop {
                    self.skip_white();
                    match self.peek() {
                        None => break,
                        Some(b'>') => {
                            self.at += 2.min(self.bytes.len() - self.at);
                            break;
                        }
                        Some(b'/') => {
                            self.at += 1;
                            let key = self.name();
                            if depth == 0 {
                                return None;
                            }
                            let value = self.value(depth - 1).unwrap_or(Pdf::Null);
                            map.insert(key, value);
                        }
                        Some(_) => {
                            // Not a key: step over it, as deep as a value.
                            if depth == 0 {
                                return None;
                            }
                            if self.value(depth - 1).is_none() {
                                self.at += 1;
                            }
                        }
                    }
                }
                Some(Pdf::Dict(map))
            }
            b'<' => {
                self.at += 1;
                Some(Pdf::Str(self.hex()))
            }
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_white();
                    match self.peek() {
                        None => break,
                        Some(b']') => {
                            self.at += 1;
                            break;
                        }
                        Some(_) => {
                            if depth == 0 {
                                return None;
                            }
                            match self.value(depth - 1) {
                                Some(value) => items.push(value),
                                None => self.at += 1,
                            }
                        }
                    }
                }
                Some(Pdf::Array(items))
            }
            b']' | b'>' | b')' | b'{' | b'}' => {
                self.at += 1;
                Some(Pdf::Op((b as char).to_string()))
            }
            b'+' | b'-' | b'.' | b'0'..=b'9' => {
                let n = self.number();
                // `n 0 R` is a reference.
                if n.fract() == 0.0 && n >= 0.0 {
                    let save = self.at;
                    self.skip_white();
                    let gen_start = self.at;
                    while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                        self.at += 1;
                    }
                    if self.at > gen_start {
                        self.skip_white();
                        if self.peek() == Some(b'R')
                            && self
                                .bytes
                                .get(self.at + 1)
                                .map_or(true, |b| is_white(*b) || is_delimiter(*b))
                        {
                            self.at += 1;
                            return Some(Pdf::Ref(n as u32));
                        }
                    }
                    self.at = save;
                }
                Some(Pdf::Num(n))
            }
            _ => {
                let start = self.at;
                while self
                    .peek()
                    .is_some_and(|b| !is_white(b) && !is_delimiter(b))
                {
                    self.at += 1;
                }
                if self.at == start {
                    self.at += 1;
                }
                let word = String::from_utf8_lossy(&self.bytes[start..self.at]).into_owned();
                Some(match word.as_str() {
                    "true" => Pdf::Bool(true),
                    "false" => Pdf::Bool(false),
                    "null" => Pdf::Null,
                    _ => Pdf::Op(word),
                })
            }
        }
    }

    fn name(&mut self) -> String {
        let mut name = Vec::new();
        while let Some(b) = self.peek() {
            if is_white(b) || is_delimiter(b) {
                break;
            }
            self.at += 1;
            if b == b'#' {
                let hex = self.bytes.get(self.at..self.at + 2);
                if let Some(code) = hex
                    .and_then(|h| std::str::from_utf8(h).ok())
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    name.push(code);
                    self.at += 2;
                    continue;
                }
            }
            name.push(b);
        }
        String::from_utf8_lossy(&name).into_owned()
    }

    fn number(&mut self) -> f64 {
        let start = self.at;
        while self
            .peek()
            .is_some_and(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.'))
        {
            self.at += 1;
        }
        let text = String::from_utf8_lossy(&self.bytes[start..self.at]);
        text.parse::<f64>().unwrap_or_else(|_| {
            // `--5`, `5.-`: what parses of it.
            text.trim_start_matches(['+', '-'])
                .split(['+', '-'])
                .next()
                .and_then(|t| t.parse().ok())
                .unwrap_or(0.0)
        })
    }

    fn literal(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut depth = 1;
        while let Some(b) = self.peek() {
            self.at += 1;
            match b {
                b'\\' => {
                    let Some(next) = self.peek() else { break };
                    self.at += 1;
                    match next {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'\r' => {
                            if self.peek() == Some(b'\n') {
                                self.at += 1;
                            }
                        }
                        b'\n' => {}
                        b'0'..=b'7' => {
                            let mut code = u32::from(next - b'0');
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(d @ b'0'..=b'7') => {
                                        code = code * 8 + u32::from(d - b'0');
                                        self.at += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push((code & 0xff) as u8);
                        }
                        other => out.push(other),
                    }
                }
                b'(' => {
                    depth += 1;
                    out.push(b);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    out.push(b);
                }
                _ => out.push(b),
            }
        }
        out
    }

    fn hex(&mut self) -> Vec<u8> {
        let mut digits = Vec::new();
        while let Some(b) = self.peek() {
            self.at += 1;
            if b == b'>' {
                break;
            }
            if b.is_ascii_hexdigit() {
                digits.push(b);
            }
        }
        if digits.len() % 2 == 1 {
            digits.push(b'0');
        }
        digits
            .chunks(2)
            .filter_map(|pair| {
                std::str::from_utf8(pair)
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
            })
            .collect()
    }
}

/// An object of a PDF: what comes before its stream, parsed, and the
/// stream's bytes as they are in the file.
struct Object {
    value: Pdf,
    stream: Option<Vec<u8>>,
}

/// Every object of a PDF by its number, the last of each number found (a
/// file updated keeps its newer objects later), with the trailers'
/// dictionaries.
fn pdf_objects(bytes: &[u8]) -> (BTreeMap<u32, Object>, Vec<Pdf>) {
    let mut objects = BTreeMap::new();
    let mut trailers = Vec::new();
    let mut search = 0;
    // Where the next `endobj` is: an object's body is read up to it, so
    // that a body never closed is not read to the end of the file for
    // every object after it.
    let mut next_end = 0usize;
    while let Some(found) = find(bytes, b"obj", search) {
        search = found + 3;
        let after_ok = bytes
            .get(found + 3)
            .map_or(true, |b| is_white(*b) || is_delimiter(*b));
        if !after_ok || bytes.get(found.wrapping_sub(3)..found) == Some(&b"end"[..]) {
            continue;
        }
        // Back over `<num> <gen> `.
        let mut at = found;
        let back_white = |at: &mut usize| {
            let start = *at;
            while *at > 0 && is_white(bytes[*at - 1]) {
                *at -= 1;
            }
            *at < start
        };
        let back_digits = |at: &mut usize| {
            let start = *at;
            while *at > 0 && bytes[*at - 1].is_ascii_digit() {
                *at -= 1;
            }
            (*at < start).then(|| String::from_utf8_lossy(&bytes[*at..start]).into_owned())
        };
        if !back_white(&mut at) || back_digits(&mut at).is_none() || !back_white(&mut at) {
            continue;
        }
        let Some(number) = back_digits(&mut at) else {
            continue;
        };
        if at > 0 && !is_white(bytes[at - 1]) && !is_delimiter(bytes[at - 1]) {
            continue;
        }
        let Ok(number) = number.parse::<u32>() else {
            continue;
        };
        let body_start = found + 3;
        if next_end < body_start {
            next_end = find(bytes, b"endobj", body_start).unwrap_or(bytes.len());
        }
        // Nor past the next object's start, where no `endobj` came first.
        let stop = find(bytes, b"obj", body_start)
            .unwrap_or(bytes.len())
            .min(next_end);
        let mut parser = Parser::new(&bytes[body_start..stop]);
        let value = parser.value(32).unwrap_or(Pdf::Null);
        let end = body_start + parser.at;
        let mut stream = None;
        let mut rest = Parser::new(&bytes[end..]);
        rest.skip_white();
        if bytes[end + rest.at..].starts_with(b"stream") {
            let mut start = end + rest.at + 6;
            if bytes.get(start) == Some(&b'\r') {
                start += 1;
            }
            if bytes.get(start) == Some(&b'\n') {
                start += 1;
            }
            let declared = value
                .get("Length")
                .and_then(Pdf::num)
                .filter(|n| *n >= 0.0 && *n <= bytes.len() as f64)
                .map(|n| n as usize)
                .filter(|&n| {
                    let Some(mut tail) = start.checked_add(n) else {
                        return false;
                    };
                    while tail < bytes.len() && is_white(bytes[tail]) {
                        tail += 1;
                    }
                    bytes
                        .get(tail..)
                        .is_some_and(|b| b.starts_with(b"endstream"))
                });
            let stop = match declared {
                Some(n) => start + n,
                None => match find(bytes, b"endstream", start) {
                    Some(mut stop) => {
                        if stop > start && bytes[stop - 1] == b'\n' {
                            stop -= 1;
                        }
                        if stop > start && bytes[stop - 1] == b'\r' {
                            stop -= 1;
                        }
                        stop
                    }
                    None => bytes.len(),
                },
            };
            stream = Some(bytes[start.min(stop)..stop].to_vec());
            search = stop;
        }
        if value.get("Type").and_then(Pdf::name) == Some("XRef") {
            trailers.push(value.clone());
        }
        objects.insert(number, Object { value, stream });
    }
    let mut search = 0;
    while let Some(found) = find(bytes, b"trailer", search) {
        search = found + 7;
        if let Some(value) = Parser::new(&bytes[found + 7..]).value(8) {
            trailers.push(value);
        }
    }
    (objects, trailers)
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + from)
}

/// How much more may still be inflated of a document.
struct Budget(usize);

/// A stream's bytes, its filters undone: none for a picture's, or one in a
/// way not read here.
fn decoded(value: &Pdf, raw: &[u8], budget: &mut Budget) -> Option<Vec<u8>> {
    let filters: Vec<String> = match value.get("Filter") {
        Some(Pdf::Name(name)) => vec![name.clone()],
        Some(Pdf::Array(items)) => items
            .iter()
            .filter_map(|i| i.name())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    };
    let params: Vec<Option<&Pdf>> = match value.get("DecodeParms") {
        Some(Pdf::Array(items)) => items.iter().map(Some).collect(),
        Some(dict) => vec![Some(dict)],
        None => Vec::new(),
    };
    let mut data = raw.to_vec();
    for (i, filter) in filters.iter().enumerate() {
        data = match filter.as_str() {
            "FlateDecode" | "Fl" => {
                let limit = MAX_INFLATED.min(budget.0);
                let inflated =
                    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&data, limit)
                        .or_else(|_| {
                            miniz_oxide::inflate::decompress_to_vec_with_limit(&data, limit)
                        })
                        .ok()?;
                budget.0 = budget.0.saturating_sub(inflated.len());
                match params.get(i).copied().flatten() {
                    Some(params) => unpredicted(inflated, params)?,
                    None => inflated,
                }
            }
            "ASCIIHexDecode" | "AHx" => Parser::new(&data).hex(),
            "ASCII85Decode" | "A85" => ascii85(&data)?,
            _ => return None,
        };
    }
    Some(data)
}

/// Data a PNG predictor was applied to, as it was before.
fn unpredicted(data: Vec<u8>, params: &Pdf) -> Option<Vec<u8>> {
    let predictor = params.get("Predictor").and_then(Pdf::num).unwrap_or(1.0) as u32;
    if predictor < 10 {
        return (predictor <= 1).then_some(data);
    }
    let colors = params.get("Colors").and_then(Pdf::num).unwrap_or(1.0) as usize;
    let bits = params
        .get("BitsPerComponent")
        .and_then(Pdf::num)
        .unwrap_or(8.0) as usize;
    let columns = params.get("Columns").and_then(Pdf::num).unwrap_or(1.0) as usize;
    let pixel = (colors * bits).div_ceil(8).max(1);
    let row = (columns * colors * bits).div_ceil(8);
    if row == 0 {
        return None;
    }
    let mut out = Vec::with_capacity(data.len());
    let mut previous = vec![0u8; row];
    for chunk in data.chunks(row + 1) {
        let (kind, line) = chunk.split_first()?;
        let mut current = line.to_vec();
        current.resize(row, 0);
        for i in 0..row {
            let left = if i >= pixel { current[i - pixel] } else { 0 };
            let up = previous[i];
            let corner = if i >= pixel { previous[i - pixel] } else { 0 };
            current[i] = match kind {
                1 => current[i].wrapping_add(left),
                2 => current[i].wrapping_add(up),
                3 => current[i].wrapping_add(((u16::from(left) + u16::from(up)) / 2) as u8),
                4 => {
                    let p = i16::from(left) + i16::from(up) - i16::from(corner);
                    let (pa, pb, pc) = (
                        (p - i16::from(left)).abs(),
                        (p - i16::from(up)).abs(),
                        (p - i16::from(corner)).abs(),
                    );
                    let near = if pa <= pb && pa <= pc {
                        left
                    } else if pb <= pc {
                        up
                    } else {
                        corner
                    };
                    current[i].wrapping_add(near)
                }
                _ => current[i],
            };
        }
        out.extend_from_slice(&current);
        previous = current;
    }
    Some(out)
}

fn ascii85(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut group = Vec::with_capacity(5);
    let data = data.strip_prefix(b"<~").unwrap_or(data);
    for b in data.iter().copied().filter(|b| !is_white(*b)) {
        if b == b'~' {
            break;
        }
        if b == b'z' && group.is_empty() {
            out.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        if !(b'!'..=b'u').contains(&b) {
            return None;
        }
        group.push(u32::from(b - b'!'));
        if group.len() == 5 {
            let n = group
                .iter()
                .fold(0u32, |n, d| n.wrapping_mul(85).wrapping_add(*d));
            out.extend_from_slice(&n.to_be_bytes());
            group.clear();
        }
    }
    if !group.is_empty() {
        let used = group.len() - 1;
        while group.len() < 5 {
            group.push(84);
        }
        let n = group
            .iter()
            .fold(0u32, |n, d| n.wrapping_mul(85).wrapping_add(*d));
        out.extend_from_slice(&n.to_be_bytes()[..used]);
    }
    Some(out)
}

/// The objects kept in object streams, added to `objects` where no object
/// of their number is there already.
fn expand_object_streams(objects: &mut BTreeMap<u32, Object>, budget: &mut Budget) {
    let streams: Vec<(Pdf, Vec<u8>)> = objects
        .values()
        .filter(|o| o.value.get("Type").and_then(Pdf::name) == Some("ObjStm"))
        .filter_map(|o| Some((o.value.clone(), o.stream.clone()?)))
        .collect();
    for (value, raw) in streams {
        let Some(data) = decoded(&value, &raw, budget) else {
            continue;
        };
        let count = value.get("N").and_then(Pdf::num).unwrap_or(0.0) as usize;
        let first = value.get("First").and_then(Pdf::num).unwrap_or(0.0) as usize;
        let mut header = Parser::new(&data[..first.min(data.len())]);
        let mut pairs = Vec::new();
        for _ in 0..count.min(100_000) {
            let (Some(Pdf::Num(n)), Some(Pdf::Num(at))) = (header.value(0), header.value(0)) else {
                break;
            };
            pairs.push((n as u32, first + at as usize));
        }
        for (i, (number, at)) in pairs.iter().enumerate() {
            let end = pairs.get(i + 1).map_or(data.len(), |(_, next)| *next);
            let Some(text) = data.get(*at..end.max(*at).min(data.len())) else {
                continue;
            };
            if objects.contains_key(number) {
                continue;
            }
            if let Some(value) = Parser::new(text).value(32) {
                objects.insert(
                    *number,
                    Object {
                        value,
                        stream: None,
                    },
                );
            }
        }
    }
}

/// What a font's codes are, as letters.
#[derive(Default)]
struct Font {
    /// Codes to text, from its `ToUnicode` map.
    map: BTreeMap<u32, String>,
    /// Bytes to a code: 2 for a composite font's.
    width: usize,
    /// Codes to text, from its encoding's differences.
    differences: BTreeMap<u32, String>,
    /// How far each code moves the pen, in thousandths of the font's size.
    advances: BTreeMap<u32, f64>,
    /// How far a code it gives no width moves it.
    default_advance: f64,
}

impl Font {
    /// How far `bytes` move the pen, in thousandths of the font's size, and
    /// how many codes and how many spaces (byte 32, in a simple font) they
    /// hold.
    fn advance(&self, bytes: &[u8]) -> (f64, usize, usize) {
        let width = self.width.max(1);
        let mut total = 0.0;
        let mut codes = 0;
        let mut spaces = 0;
        for chunk in bytes.chunks(width) {
            let code = chunk.iter().fold(0u32, |n, b| n << 8 | u32::from(*b));
            total += self
                .advances
                .get(&code)
                .copied()
                .unwrap_or(self.default_advance);
            codes += 1;
            if width == 1 && code == 32 {
                spaces += 1;
            }
        }
        (total, codes, spaces)
    }

    /// The text `bytes` show in this font, and how many codes had no letter.
    fn text(&self, bytes: &[u8]) -> (String, usize) {
        let mut out = String::new();
        let mut unknown = 0;
        let width = self.width.max(1);
        for chunk in bytes.chunks(width) {
            let code = chunk.iter().fold(0u32, |n, b| n << 8 | u32::from(*b));
            if let Some(text) = self.map.get(&code).or_else(|| self.differences.get(&code)) {
                out.push_str(text);
            } else if width == 1 {
                // No map, or a map that leaves the code out: as WinAnsi.
                out.push(win_ansi(code as u8));
            } else {
                unknown += 1;
            }
        }
        (out, unknown)
    }
}

/// A byte of the WinAnsi encoding, as a letter.
fn win_ansi(b: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}',
        'ž', 'Ÿ',
    ];
    match b {
        0x80..=0x9f => HIGH[(b - 0x80) as usize],
        _ => b as char,
    }
}

/// Text from bytes in UTF-16BE, as a `ToUnicode` map gives it.
fn utf16(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks(2)
        .map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]))
        .collect();
    char::decode_utf16(units)
        .map(|c| c.unwrap_or('\u{fffd}'))
        .collect()
}

/// A `ToUnicode` map: codes to text, and the width of a code in bytes.
fn to_unicode(data: &[u8]) -> (BTreeMap<u32, String>, usize) {
    let mut map = BTreeMap::new();
    let mut width = 0;
    let mut parser = Parser::new(data);
    let code = |bytes: &[u8]| bytes.iter().fold(0u32, |n, b| n << 8 | u32::from(*b));
    let mut operands: Vec<Pdf> = Vec::new();
    let mut section = "";
    while let Some(token) = parser.value(4) {
        match token {
            Pdf::Op(op) => {
                match op.as_str() {
                    "begincodespacerange" => section = "space",
                    "beginbfchar" => section = "char",
                    "beginbfrange" => section = "range",
                    "endcodespacerange" | "endbfchar" | "endbfrange" => {
                        match section {
                            "space" => {
                                if let Some(Pdf::Str(low)) = operands.first() {
                                    width = width.max(low.len());
                                }
                            }
                            "char" => {
                                for pair in operands.chunks(2) {
                                    if let [Pdf::Str(src), Pdf::Str(dst)] = pair {
                                        width = width.max(src.len());
                                        map.insert(code(src), utf16(dst));
                                    }
                                }
                            }
                            "range" => {
                                for triple in operands.chunks(3) {
                                    let [Pdf::Str(low), Pdf::Str(high), dst] = triple else {
                                        continue;
                                    };
                                    width = width.max(low.len());
                                    let (low, high) = (code(low), code(high));
                                    if high < low || high - low > 65_535 || map.len() > 200_000 {
                                        continue;
                                    }
                                    match dst {
                                        Pdf::Str(first) => {
                                            let mut units = first.clone();
                                            for c in low..=high {
                                                map.insert(c, utf16(&units));
                                                // The last unit goes up by one.
                                                if let Some(last) = units.last_mut() {
                                                    let (bumped, carry) = last.overflowing_add(1);
                                                    *last = bumped;
                                                    if carry && units.len() >= 2 {
                                                        let i = units.len() - 2;
                                                        units[i] = units[i].wrapping_add(1);
                                                    }
                                                }
                                            }
                                        }
                                        Pdf::Array(each) => {
                                            for (c, item) in (low..=high).zip(each) {
                                                if let Pdf::Str(text) = item {
                                                    map.insert(c, utf16(text));
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            _ => {}
                        }
                        section = "";
                    }
                    _ => {}
                }
                operands.clear();
            }
            other if !section.is_empty() => operands.push(other),
            _ => {}
        }
    }
    (map, width)
}

/// Glyph names to letters, for the names `Differences` arrays use most.
fn glyph(name: &str) -> Option<String> {
    if let Some(hex) = name.strip_prefix("uni").filter(|h| h.len() == 4) {
        return u32::from_str_radix(hex, 16)
            .ok()
            .and_then(char::from_u32)
            .map(|c| c.to_string());
    }
    if name.len() == 1 && name.is_ascii() {
        return Some(name.to_string());
    }
    Some(
        match name {
            "space" => " ",
            "quoteright" => "’",
            "quoteleft" => "‘",
            "quotedblleft" => "“",
            "quotedblright" => "”",
            "quotesingle" => "'",
            "quotedbl" => "\"",
            "bullet" => "•",
            "endash" => "–",
            "emdash" => "—",
            "hyphen" => "-",
            "period" => ".",
            "comma" => ",",
            "colon" => ":",
            "semicolon" => ";",
            "exclam" => "!",
            "question" => "?",
            "parenleft" => "(",
            "parenright" => ")",
            "slash" => "/",
            "ampersand" => "&",
            "percent" => "%",
            "dollar" => "$",
            "at" => "@",
            "fi" => "fi",
            "fl" => "fl",
            "ff" => "ff",
            "ffi" => "ffi",
            "ffl" => "ffl",
            "ellipsis" => "…",
            "degree" => "°",
            "copyright" => "©",
            "registered" => "®",
            "trademark" => "™",
            "zero" => "0",
            "one" => "1",
            "two" => "2",
            "three" => "3",
            "four" => "4",
            "five" => "5",
            "six" => "6",
            "seven" => "7",
            "eight" => "8",
            "nine" => "9",
            _ => return None,
        }
        .to_string(),
    )
}

/// What reading a PDF found and needs as it goes.
struct Reader<'a> {
    objects: &'a BTreeMap<u32, Object>,
    budget: Budget,
    fonts: BTreeMap<u32, std::rc::Rc<Font>>,
    unknown: usize,
    /// The operators of content streams left to work through, forms drawn
    /// within others included: a form drawn by itself many times over is
    /// cut short, not followed for ever.
    operators_left: usize,
}

/// The most operators of content streams a PDF is read through.
pub const MAX_OPERATORS: usize = 4_000_000;

impl Reader<'_> {
    /// `value`, followed if it is a reference.
    fn resolved<'v>(&'v self, value: &'v Pdf) -> &'v Pdf {
        let mut value = value;
        for _ in 0..8 {
            match value {
                Pdf::Ref(n) => match self.objects.get(n) {
                    Some(object) => value = &object.value,
                    None => return &NULL,
                },
                _ => return value,
            }
        }
        &NULL
    }

    fn stream(&mut self, value: &Pdf) -> Option<Vec<u8>> {
        let Pdf::Ref(n) = value else {
            return None;
        };
        let object = self.objects.get(n)?;
        let raw = object.stream.as_ref()?;
        decoded(&object.value, raw, &mut self.budget)
    }

    fn font(&mut self, reference: &Pdf) -> std::rc::Rc<Font> {
        let id = match reference {
            Pdf::Ref(n) => Some(*n),
            _ => None,
        };
        if let Some(cached) = id.and_then(|id| self.fonts.get(&id)) {
            return cached.clone();
        }
        let dict = self.resolved(reference).clone();
        let composite = dict.get("Subtype").and_then(Pdf::name) == Some("Type0");
        let mut font = Font {
            width: if composite { 2 } else { 1 },
            default_advance: 500.0,
            ..Font::default()
        };
        // How far each code moves the pen: a simple font's `Widths` from its
        // `FirstChar`, a composite one's `W` and `DW`.
        if composite {
            let descendant = match self.resolved(dict.get("DescendantFonts").unwrap_or(&NULL)) {
                Pdf::Array(items) => items.first().map(|d| self.resolved(d).clone()),
                _ => None,
            }
            .unwrap_or(Pdf::Null);
            font.default_advance = descendant.get("DW").and_then(Pdf::num).unwrap_or(1000.0);
            if let Pdf::Array(items) = self.resolved(descendant.get("W").unwrap_or(&NULL)) {
                let mut i = 0;
                while i < items.len() && font.advances.len() < 100_000 {
                    match (items.get(i).and_then(Pdf::num), items.get(i + 1)) {
                        (Some(first), Some(Pdf::Array(each))) => {
                            for (k, w) in each.iter().enumerate() {
                                if let Some(w) = self.resolved(w).num() {
                                    font.advances.insert(first as u32 + k as u32, w);
                                }
                            }
                            i += 2;
                        }
                        (Some(first), Some(last)) => {
                            let last = last.num().unwrap_or(first);
                            let w = items
                                .get(i + 2)
                                .and_then(Pdf::num)
                                .unwrap_or(font.default_advance);
                            let (first, last) = (first as u32, last as u32);
                            if last >= first && last - first < 65_536 {
                                for code in first..=last {
                                    font.advances.insert(code, w);
                                }
                            }
                            i += 3;
                        }
                        _ => break,
                    }
                }
            }
        } else {
            let first = dict.get("FirstChar").and_then(Pdf::num).unwrap_or(0.0) as u32;
            if let Pdf::Array(widths) = self.resolved(dict.get("Widths").unwrap_or(&NULL)) {
                for (k, w) in widths.iter().take(65_536).enumerate() {
                    if let Some(w) = self.resolved(w).num() {
                        font.advances.insert(first + k as u32, w);
                    }
                }
            }
            let descriptor = self.resolved(dict.get("FontDescriptor").unwrap_or(&NULL));
            if let Some(missing) = descriptor.get("MissingWidth").and_then(Pdf::num) {
                font.default_advance = missing;
            } else if let Some(Pdf::Name(base)) = dict.get("BaseFont") {
                if base.contains("Courier") {
                    font.default_advance = 600.0;
                }
            }
        }
        if let Some(map) = dict.get("ToUnicode").cloned() {
            if let Some(data) = self.stream(&map) {
                let (map, width) = to_unicode(&data);
                font.map = map;
                if width > 0 {
                    font.width = width;
                }
            }
        }
        let encoding = self.resolved(dict.get("Encoding").unwrap_or(&NULL)).clone();
        {
            if let Some(Pdf::Array(items)) = encoding.get("Differences") {
                let mut code = 0u32;
                for item in items {
                    match item {
                        Pdf::Num(n) => code = *n as u32,
                        Pdf::Name(name) => {
                            if let Some(text) = glyph(name) {
                                font.differences.insert(code, text);
                            }
                            code += 1;
                        }
                        _ => {}
                    }
                }
            }
        }
        let font = std::rc::Rc::new(font);
        if let Some(id) = id {
            self.fonts.insert(id, font.clone());
        }
        font
    }

    /// The text a content stream draws, with the page's `resources`, forms
    /// it draws read `depth` deep at most. Where the pen is, as far as the
    /// fonts' widths tell, decides where a word ends and a line breaks.
    fn content(&mut self, data: &[u8], resources: &Pdf, depth: usize, lines: &mut Lines) {
        let resources = self.resolved(resources).clone();
        let fonts = self
            .resolved(resources.get("Font").unwrap_or(&NULL))
            .clone();
        let xobjects = self
            .resolved(resources.get("XObject").unwrap_or(&NULL))
            .clone();
        let mut parser = Parser::new(data);
        let mut operands: Vec<Pdf> = Vec::new();
        let mut pen = Pen::default();
        while let Some(token) = parser.value(16) {
            if self.operators_left == 0 {
                return;
            }
            self.operators_left -= 1;
            let Pdf::Op(op) = token else {
                operands.push(token);
                if operands.len() > 64 {
                    operands.remove(0);
                }
                continue;
            };
            let num = |i: usize| -> f64 {
                operands
                    .len()
                    .checked_sub(i + 1)
                    .and_then(|at| operands[at].num())
                    .unwrap_or(0.0)
            };
            match op.as_str() {
                "Tf" => {
                    if let Some(Pdf::Name(name)) =
                        operands.len().checked_sub(2).map(|i| &operands[i])
                    {
                        if let Some(reference) = fonts.get(name) {
                            pen.font = Some(self.font(reference));
                        }
                    }
                    pen.size = num(0);
                }
                "Tc" => pen.char_spacing = num(0),
                "Tw" => pen.word_spacing = num(0),
                "Tz" => pen.scaling = num(0) / 100.0,
                "TL" => pen.leading = num(0),
                "BT" => pen.begin(),
                "ET" => {}
                "Tm" => pen.matrix(num(5), num(4), num(2), num(1), num(0)),
                "Td" => pen.move_by(num(1), num(0)),
                "TD" => {
                    pen.leading = -num(0);
                    pen.move_by(num(1), num(0));
                }
                "T*" => {
                    pen.move_by(0.0, -pen.leading);
                    lines.newline();
                }
                "Tj" | "'" | "\"" => {
                    if op != "Tj" {
                        pen.move_by(0.0, -pen.leading);
                        lines.newline();
                    }
                    if let Some(Pdf::Str(bytes)) = operands.last() {
                        self.show(bytes, &mut pen, lines);
                    }
                }
                "TJ" => {
                    if let Some(Pdf::Array(items)) = operands.last() {
                        for item in items {
                            match item {
                                Pdf::Str(bytes) => self.show(bytes, &mut pen, lines),
                                Pdf::Num(n) => {
                                    // A move right of more than about a sixth of
                                    // the font's size is a space between words.
                                    if *n < -160.0 {
                                        lines.space();
                                    }
                                    pen.x -= n / 1000.0 * pen.size * pen.scaling * pen.a;
                                }
                                _ => {}
                            }
                        }
                    }
                }
                "Do" if depth < 4 => {
                    if let Some(Pdf::Name(name)) = operands.last() {
                        if let Some(reference) = xobjects.get(name).cloned() {
                            let form = self.resolved(&reference).clone();
                            if form.get("Subtype").and_then(Pdf::name) == Some("Form") {
                                if let Some(data) = self.stream(&reference) {
                                    let inner = form
                                        .get("Resources")
                                        .cloned()
                                        .unwrap_or_else(|| resources.clone());
                                    self.content(&data, &inner, depth + 1, lines);
                                }
                            }
                        }
                    }
                }
                "BI" => {
                    // An inline picture: up to `EI`, past its bytes.
                    match find(data, b"ID", parser.at) {
                        Some(start) => {
                            let mut at = start + 2;
                            loop {
                                match find(data, b"EI", at) {
                                    Some(end)
                                        if end > 0
                                            && is_white(data[end - 1])
                                            && data.get(end + 2).map_or(true, |b| is_white(*b)) =>
                                    {
                                        parser.at = end + 2;
                                        break;
                                    }
                                    Some(end) => at = end + 2,
                                    None => {
                                        parser.at = data.len();
                                        break;
                                    }
                                }
                            }
                        }
                        None => parser.at = data.len(),
                    }
                }
                _ => {}
            }
            operands.clear();
        }
    }

    fn show(&mut self, bytes: &[u8], pen: &mut Pen, lines: &mut Lines) {
        let (text, unknown) = match pen.font.as_deref() {
            Some(font) => font.text(bytes),
            None => (bytes.iter().map(|b| win_ansi(*b)).collect(), 0),
        };
        self.unknown += unknown;
        pen.gap(lines);
        lines.text(&text);
        let (advance, codes, spaces) = match pen.font.as_deref() {
            Some(font) => font.advance(bytes),
            None => (500.0 * bytes.len() as f64, bytes.len(), 0),
        };
        let spacing = pen.char_spacing * codes as f64 + pen.word_spacing * spaces as f64;
        pen.x += (advance / 1000.0 * pen.size + spacing) * pen.scaling * pen.a;
        pen.drawn = true;
    }
}

/// Where the pen is as text is drawn, in the page's space: the start of
/// the line, the end of what was drawn last, and the text state.
struct Pen {
    font: Option<std::rc::Rc<Font>>,
    size: f64,
    char_spacing: f64,
    word_spacing: f64,
    scaling: f64,
    leading: f64,
    /// The text matrix's scale, across and up.
    a: f64,
    d: f64,
    /// The start of the line.
    line_x: f64,
    line_y: f64,
    /// The end of what was drawn last, and the height of its line.
    x: f64,
    last_y: f64,
    /// Whether anything was drawn yet.
    drawn: bool,
    /// Where the next text starts, if the pen moved since the last.
    moved_to: Option<f64>,
}

impl Default for Pen {
    fn default() -> Pen {
        Pen {
            font: None,
            size: 10.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            scaling: 1.0,
            leading: 0.0,
            a: 1.0,
            d: 1.0,
            line_x: 0.0,
            line_y: 0.0,
            x: 0.0,
            last_y: 0.0,
            drawn: false,
            moved_to: None,
        }
    }
}

impl Pen {
    /// A text object begins: its matrix is the page's.
    fn begin(&mut self) {
        self.a = 1.0;
        self.d = 1.0;
        self.line_x = 0.0;
        self.line_y = 0.0;
        self.moved_to = Some(0.0);
    }

    /// The font's size on the page.
    fn em(&self) -> f64 {
        (self.size * self.d).abs().max(1.0)
    }

    fn matrix(&mut self, a: f64, b: f64, d: f64, e: f64, f: f64) {
        self.a = (a * a + b * b).sqrt().max(1e-6);
        self.d = d;
        self.line_x = e;
        self.line_y = f;
        self.moved_to = Some(e);
    }

    fn move_by(&mut self, tx: f64, ty: f64) {
        self.line_x += tx * self.a;
        self.line_y += ty * self.d;
        self.moved_to = Some(self.line_x);
    }

    /// Before text is drawn: a new line if the pen is on another line than
    /// what was drawn last, a space if it moved to leave a gap after it.
    fn gap(&mut self, lines: &mut Lines) {
        if self.drawn && (self.line_y - self.last_y).abs() > 0.5 * self.em() {
            lines.newline();
        } else if let Some(to) = self.moved_to {
            if self.drawn && to - self.x > 0.15 * self.em() {
                lines.space();
            }
        }
        if let Some(to) = self.moved_to.take() {
            self.x = to;
        }
        self.last_y = self.line_y;
    }
}

/// Lines of text being drawn.
#[derive(Default)]
struct Lines {
    lines: Vec<String>,
    current: String,
    space: bool,
}

impl Lines {
    fn text(&mut self, text: &str) {
        for c in text.chars() {
            if c.is_whitespace() || c.is_control() {
                self.space = true;
                continue;
            }
            if self.space && !self.current.is_empty() {
                self.current.push(' ');
            }
            self.space = false;
            self.current.push(c);
        }
    }

    fn space(&mut self) {
        self.space = true;
    }

    fn newline(&mut self) {
        let line = std::mem::take(&mut self.current);
        if !line.trim().is_empty() {
            self.lines.push(line.trim().to_string());
        }
        self.space = false;
    }

    fn done(mut self) -> Vec<String> {
        self.newline();
        self.lines
    }
}

/// A PDF's text, page by page, each page under `[page N]`. A PDF that is
/// encrypted, or whose fonts do not say which letters they draw, is
/// refused with why; one with no text at all (a scan) gives none.
pub fn pdf_to_text(bytes: &[u8]) -> Result<String, String> {
    let head = &bytes[..bytes.len().min(1024)];
    if !head.windows(5).any(|w| w == b"%PDF-") {
        return Err("not a PDF".into());
    }
    let (mut objects, trailers) = pdf_objects(bytes);
    if objects.is_empty() {
        return Err("no objects were found in it: the PDF may be damaged".into());
    }
    if trailers.iter().any(|t| t.get("Encrypt").is_some()) {
        return Err("the PDF is encrypted: its text cannot be read here".into());
    }
    let mut budget = Budget(MAX_INFLATED_ALL);
    expand_object_streams(&mut objects, &mut budget);
    let mut reader = Reader {
        objects: &objects,
        budget,
        fonts: BTreeMap::new(),
        unknown: 0,
        operators_left: MAX_OPERATORS,
    };
    // The pages in order: from the catalog's page tree, else every page.
    let root = trailers
        .iter()
        .rev()
        .find_map(|t| t.get("Root").cloned())
        .or_else(|| {
            objects
                .iter()
                .find(|(_, o)| o.value.get("Type").and_then(Pdf::name) == Some("Catalog"))
                .map(|(n, _)| Pdf::Ref(*n))
        });
    let mut pages: Vec<(u32, Pdf)> = Vec::new();
    if let Some(root) = root {
        let catalog = reader.resolved(&root).clone();
        if let Some(tree) = catalog.get("Pages").cloned() {
            let mut seen = BTreeSet::new();
            walk_pages(&reader, &tree, &Pdf::Null, &mut pages, &mut seen, 0);
        }
    }
    if pages.is_empty() {
        pages = objects
            .iter()
            .filter(|(_, o)| o.value.get("Type").and_then(Pdf::name) == Some("Page"))
            .map(|(n, o)| (*n, o.value.get("Resources").cloned().unwrap_or(Pdf::Null)))
            .collect();
    }
    if pages.is_empty() {
        return Err("it has no pages that could be found".into());
    }
    let mut out = Vec::new();
    for (i, (page, resources)) in pages.iter().take(MAX_PAGES).enumerate() {
        let value = reader.resolved(&Pdf::Ref(*page)).clone();
        let contents: Vec<Pdf> = match value.get("Contents") {
            Some(Pdf::Array(items)) => items.clone(),
            Some(single) => match reader.resolved(single) {
                Pdf::Array(items) => items.clone(),
                _ => vec![single.clone()],
            },
            None => Vec::new(),
        };
        let mut data = Vec::new();
        for part in &contents {
            if let Some(bytes) = reader.stream(part) {
                data.extend_from_slice(&bytes);
                data.push(b'\n');
            }
        }
        let mut lines = Lines::default();
        reader.content(&data, resources, 0, &mut lines);
        let text = lines.done().join("\n");
        out.push(format!("[page {}]\n{text}", i + 1));
    }
    let drawn: usize = out
        .iter()
        .map(|page| page.lines().skip(1).map(str::len).sum::<usize>())
        .sum();
    if drawn == 0 && reader.unknown > 0 {
        return Err(
            "its text is drawn in fonts that do not say which letters they show (no ToUnicode \
             map): it would have to be read from pictures of its pages"
                .into(),
        );
    }
    if pages.len() > MAX_PAGES {
        out.push(format!("[… {} pages more]", pages.len() - MAX_PAGES));
    }
    Ok(out.join("\n\n"))
}

/// The pages under `node` of a page tree, in order, each with the
/// resources it has or inherits.
fn walk_pages(
    reader: &Reader<'_>,
    node: &Pdf,
    inherited: &Pdf,
    pages: &mut Vec<(u32, Pdf)>,
    seen: &mut BTreeSet<u32>,
    depth: usize,
) {
    let Pdf::Ref(id) = node else {
        return;
    };
    if depth > 64 || !seen.insert(*id) || pages.len() > MAX_PAGES {
        return;
    }
    let value = reader.resolved(node);
    let resources = value
        .get("Resources")
        .cloned()
        .unwrap_or_else(|| inherited.clone());
    match value.get("Type").and_then(Pdf::name) {
        Some("Page") => pages.push((*id, resources)),
        _ => {
            if let Some(Pdf::Array(kids)) = value.get("Kids").map(|k| reader.resolved(k)) {
                for kid in kids.clone() {
                    walk_pages(reader, &kid, &resources, pages, seen, depth + 1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_web_page_is_read_as_its_text() {
        let html = r##"<!DOCTYPE html><html><head><title>The  Guide &amp; more</title>
            <style>body { color: red }</style><script>alert("no")</script></head>
            <body><nav><a href="/">Home</a> <a href="/about">About</a></nav>
            <h1>The Guide</h1>
            <p>Read the <a href="docs/start.html">start page</a>, then <b>the rest</b>.<br>A new line.</p>
            <ul><li>one</li><li>two <a href="https://example.org/x">https://example.org/x</a></li></ul>
            <ol start="3"><li>third</li><li>fourth</li></ol>
            <table><tr><th>Name</th><th>Price</th></tr><tr><td>Tea</td><td>&euro;3</td></tr></table>
            <pre>  keep
    this</pre>
            <img src="a.png" alt="A chart of sales"><a href="#top">Top</a>
            <footer>Copyright</footer></body></html>"##;
        let text = html_to_text(html, Some("https://site.example/guide/index.html"));
        assert_eq!(
            text,
            "The Guide & more\n\n# The Guide\n\nRead the [start page](https://site.example/guide/docs/start.html), then the rest.\nA new line.\n\n- one\n- two https://example.org/x\n\n3. third\n4. fourth\n\nName | Price\nTea | €3\n\n  keep\n    this\n\n[image: A chart of sales] Top"
        );
    }

    #[test]
    fn links_are_made_whole_from_where_the_page_came_from() {
        let base = Some("https://a.example/b/c/page?q=1");
        assert_eq!(
            resolve(base, "d.html").unwrap(),
            "https://a.example/b/c/d.html"
        );
        assert_eq!(resolve(base, "../d").unwrap(), "https://a.example/b/d");
        assert_eq!(resolve(base, "/root").unwrap(), "https://a.example/root");
        assert_eq!(
            resolve(base, "//cdn.example/x").unwrap(),
            "https://cdn.example/x"
        );
        assert_eq!(
            resolve(base, "?p=2").unwrap(),
            "https://a.example/b/c/page?p=2"
        );
        assert_eq!(
            resolve(base, "./e/../f#s").unwrap(),
            "https://a.example/b/c/f#s"
        );
        assert_eq!(resolve(base, "http://other/x").unwrap(), "http://other/x");
        assert_eq!(
            resolve(Some("https://a.example"), "x").unwrap(),
            "https://a.example/x"
        );
        assert_eq!(resolve(None, "x"), None);
        assert_eq!(
            decode_entities("a &lt;b&gt; &#233;&#xE9; &bogus; &"),
            "a <b> éé &bogus; &"
        );
    }

    #[test]
    fn what_bytes_are_is_told_by_how_they_begin() {
        assert_eq!(kind_of(b"%PDF-1.7\n", "x", None), Kind::Pdf);
        assert_eq!(
            kind_of(b"\x89PNG\r\n\x1a\n....", "x", None),
            Kind::Picture("image/png")
        );
        assert_eq!(kind_of(b"  <!DOCTYPE html><p>", "x", None), Kind::Html);
        assert_eq!(kind_of(b"<p>hi</p>", "page.htm", None), Kind::Html);
        assert_eq!(
            kind_of(b"<p>hi</p>", "x", Some("text/html; charset=utf-8")),
            Kind::Html
        );
        assert_eq!(kind_of(b"{\"a\": 1}", "x.json", None), Kind::Text);
        assert_eq!(kind_of(&[0, 159, 146, 150], "x", None), Kind::Binary);
    }

    /// A ZIP archive of `files`, stored or deflated.
    fn zip(files: &[(&str, &str, bool)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut directory = Vec::new();
        for (name, content, deflate) in files {
            let data = if *deflate {
                miniz_oxide::deflate::compress_to_vec(content.as_bytes(), 6)
            } else {
                content.as_bytes().to_vec()
            };
            let offset = out.len() as u32;
            let method: u16 = if *deflate { 8 } else { 0 };
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&[20, 0, 0, 0]);
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0; 8]);
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(content.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&data);
            directory.extend_from_slice(b"PK\x01\x02");
            directory.extend_from_slice(&[20, 0, 20, 0, 0, 0]);
            directory.extend_from_slice(&method.to_le_bytes());
            directory.extend_from_slice(&[0; 8]);
            directory.extend_from_slice(&(data.len() as u32).to_le_bytes());
            directory.extend_from_slice(&(content.len() as u32).to_le_bytes());
            directory.extend_from_slice(&(name.len() as u16).to_le_bytes());
            directory.extend_from_slice(&[0; 12]);
            directory.extend_from_slice(&offset.to_le_bytes());
            directory.extend_from_slice(name.as_bytes());
        }
        let at = out.len() as u32;
        out.extend_from_slice(&directory);
        out.extend_from_slice(b"PK\x05\x06");
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(directory.len() as u32).to_le_bytes());
        out.extend_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    #[test]
    fn a_spreadsheet_is_read_as_its_sheets_rows() {
        let book = zip(&[
            (
                "xl/workbook.xml",
                r#"<workbook xmlns:r="r"><sheets><sheet name="Sales" sheetId="1" r:id="rId1"/><sheet name="Notes" sheetId="2" r:id="rId2"/></sheets></workbook>"#,
                true,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<Relationships><Relationship Id="rId1" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Target="/xl/worksheets/notes.xml"/></Relationships>"#,
                false,
            ),
            (
                "xl/sharedStrings.xml",
                r#"<sst><si><t>Region</t></si><si><t>Total</t></si><si><r><t>No</t></r><r><t>rth</t></r><rPh><t>x</t></rPh></si></sst>"#,
                true,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row><row r="3"><c r="A3" t="s"><v>2</v></c><c r="C3"><v>12.5</v></c><c r="D3" t="b"><v>1</v></c><c r="E3"><f>C3*2</f></c></row></sheetData></worksheet>"#,
                true,
            ),
            (
                "xl/worksheets/notes.xml",
                r#"<worksheet><sheetData><row r="1"><c r="B1" t="inlineStr"><is><t>a &amp; b</t></is></c></row></sheetData></worksheet>"#,
                false,
            ),
        ]);
        assert_eq!(kind_of(&book, "x", None), Kind::Xlsx);
        assert_eq!(
            xlsx_to_text(&book).unwrap(),
            "## Sheet: Sales (2 rows)\nrow\tA\tB\tC\tD\tE\n1\tRegion\tTotal\n3\tNorth\t\t12.5\tTRUE\t=C3*2\n\n## Sheet: Notes (1 row)\nrow\tA\tB\n1\t\ta & b"
        );
        assert_eq!(column_of("AB12"), 28);
        assert_eq!(column_name(28), "AB");
    }

    #[test]
    fn a_document_and_a_deck_are_read_as_their_words() {
        let doc = zip(&[(
            "word/document.xml",
            r#"<w:document><w:body><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Plan</w:t></w:r></w:p><w:p><w:r><w:t xml:space="preserve">First </w:t></w:r><w:r><w:t>step.</w:t></w:r></w:p><w:p><w:pPr><w:numPr/></w:pPr><w:r><w:t>Item</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>"#,
            true,
        )]);
        assert_eq!(kind_of(&doc, "x", None), Kind::Docx);
        assert_eq!(
            docx_to_text(&doc).unwrap(),
            "# Plan\nFirst step.\n- Item\na | b"
        );
        let deck = zip(&[
            ("ppt/presentation.xml", "<p:presentation/>", false),
            ("ppt/slides/slide2.xml", "<p:sld><a:p><a:r><a:t>Second</a:t></a:r></a:p></p:sld>", true),
            ("ppt/slides/slide10.xml", "<p:sld><a:p><a:r><a:t>Tenth</a:t></a:r></a:p></p:sld>", true),
            ("ppt/slides/slide1.xml", "<p:sld><a:p><a:r><a:t>Title</a:t></a:r></a:p><a:p><a:r><a:t>Sub</a:t></a:r></a:p></p:sld>", true),
        ]);
        assert_eq!(kind_of(&deck, "x", None), Kind::Pptx);
        assert_eq!(
            pptx_to_text(&deck).unwrap(),
            "## Slide 1\nTitle\nSub\n\n## Slide 2\nSecond\n\n## Slide 10\nTenth"
        );
    }

    /// A PDF of `objects` (each the text after `n 0 obj`), the first the
    /// catalog.
    fn pdf(objects: &[Vec<u8>]) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        for (i, object) in objects.iter().enumerate() {
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(object);
            out.extend_from_slice(b"\nendobj\n");
        }
        out.extend_from_slice(b"trailer\n<< /Root 1 0 R /Size 9 >>\n%%EOF\n");
        out
    }

    fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
        let mut out = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
        out.extend_from_slice(data);
        out.extend_from_slice(b"\nendstream");
        out
    }

    #[test]
    fn a_pdf_is_read_page_by_page_through_its_fonts_maps() {
        let page_one = b"BT /F1 12 Tf 72 720 Td (Hello, world) Tj 0 -14 Td [(Sec) -20 (ond) -300 (line)] TJ ET";
        let flated = miniz_oxide::deflate::compress_to_vec_zlib(page_one, 6);
        // The second page's font is composite, its codes mapped to letters.
        let page_two =
            b"BT /F2 10 Tf 1 0 0 1 72 700 Tm <00010002> Tj 1 0 0 1 72 680 Tm <0003> Tj ET";
        let cmap = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n1 begincodespacerange <0000> <FFFF> endcodespacerange\n2 beginbfchar <0001> <0048> <0002> <0069> endbfchar\n1 beginbfrange <0003> <0003> <00E9> endbfrange\nendcmap end end";
        let objects = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 /Resources << /Font << /F1 7 0 R /F2 8 0 R >> >> >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
            stream("/Filter /FlateDecode", &flated),
            b"<< /Type /Page /Parent 2 0 R /Contents [6 0 R] >>".to_vec(),
            stream("", page_two),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            b"<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /Identity-H /ToUnicode 9 0 R >>".to_vec(),
            stream("", cmap),
        ];
        let bytes = pdf(&objects);
        assert_eq!(kind_of(&bytes, "x", None), Kind::Pdf);
        assert_eq!(
            pdf_to_text(&bytes).unwrap(),
            "[page 1]\nHello, world\nSecond line\n\n[page 2]\nHi\né"
        );
    }

    #[test]
    fn a_pdf_that_cannot_be_read_says_why() {
        assert_eq!(pdf_to_text(b"not one").unwrap_err(), "not a PDF");
        let encrypted = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n<< /Root 1 0 R /Encrypt 2 0 R >>\n";
        assert!(pdf_to_text(encrypted).unwrap_err().contains("encrypted"));
        // Codes of a composite font with no map to letters.
        let objects = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
            stream("", b"BT /F1 9 Tf <00410042> Tj ET"),
            b"<< /Type /Font /Subtype /Type0 /Encoding /Identity-H >>".to_vec(),
        ];
        assert!(pdf_to_text(&pdf(&objects))
            .unwrap_err()
            .contains("no ToUnicode map"));
    }

    #[test]
    fn objects_kept_in_object_streams_are_found() {
        // The catalog, the page tree and the page live in an object stream.
        let kept = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>",
        ];
        let mut header = String::new();
        let mut body = String::new();
        for (i, object) in kept.iter().enumerate() {
            header.push_str(&format!("{} {} ", i + 1, body.len()));
            body.push_str(object);
            body.push(' ');
        }
        let first = header.len();
        let packed =
            miniz_oxide::deflate::compress_to_vec_zlib(format!("{header}{body}").as_bytes(), 6);
        let mut bytes = b"%PDF-1.5\n".to_vec();
        bytes.extend_from_slice(b"4 0 obj\n");
        bytes.extend_from_slice(&stream("", b"BT (Packed away) Tj ET"));
        bytes.extend_from_slice(b"\nendobj\n5 0 obj\n");
        bytes.extend_from_slice(&stream(
            &format!("/Type /ObjStm /N 3 /First {first} /Filter /FlateDecode"),
            &packed,
        ));
        bytes.extend_from_slice(
            b"\nendobj\n6 0 obj\n<< /Type /XRef /Root 1 0 R /Size 7 >>\nendobj\n",
        );
        let text = pdf_to_text(&bytes).unwrap();
        assert_eq!(text, "[page 1]\nPacked away");
    }

    /// Runs `read` on another thread with a deep stack's worth of room
    /// taken away: it must come back within `seconds`.
    fn quickly<T: Send + 'static>(seconds: u64, read: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(move || {
                let _ = tx.send(read());
            })
            .unwrap();
        rx.recv_timeout(std::time::Duration::from_secs(seconds))
            .expect("reading took too long or crashed")
    }

    #[test]
    fn a_pdf_made_to_break_the_reader_does_not() {
        // Dictionaries opened a hundred thousand deep.
        let mut deep = b"%PDF-1.7\n1 0 obj\n".to_vec();
        deep.extend(std::iter::repeat(b"<< ").take(100_000).flatten());
        let _ = quickly(20, move || pdf_to_text(&deep));
        // A length past any number.
        let huge = pdf(&[
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Length 99999999999999999999999 >>\nstream\nBT ET\nendstream".to_vec(),
        ]);
        let _ = quickly(20, move || pdf_to_text(&huge));
        // Arrays never closed, one per object.
        let mut open = b"%PDF-1.7\n".to_vec();
        for i in 1..30_000 {
            open.extend_from_slice(format!("{i} 0 obj [ ").as_bytes());
        }
        let _ = quickly(20, move || pdf_to_text(&open));
        // A form that draws itself eighty times over.
        let draws = "/X Do ".repeat(80);
        let looped = pdf(&[
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /X 5 0 R >> /Font << /F1 6 0 R >> >> >>".to_vec(),
            stream("", format!("BT /F1 12 Tf (top) Tj ET {draws}").as_bytes()),
            stream(
                "/Type /XObject /Subtype /Form /Resources << /XObject << /X 5 0 R >> /Font << /F1 6 0 R >> >>",
                format!("BT /F1 12 Tf (again) Tj ET {draws}").as_bytes(),
            ),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        ]);
        let text = quickly(60, move || pdf_to_text(&looped)).unwrap();
        assert!(
            text.starts_with("[page 1]\ntop"),
            "{}",
            &text[..40.min(text.len())]
        );
    }

    #[test]
    fn a_workbook_with_cells_far_apart_stays_small() {
        let mut rows = String::new();
        for r in 1..=20_000 {
            rows.push_str(&format!(r#"<row r="{r}"><c r="XFD{r}"><v>1</v></c></row>"#));
        }
        let book = zip(&[
            (
                "xl/workbook.xml",
                r#"<workbook xmlns:r="r"><sheets><sheet name="Wide" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
                false,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<Relationships><Relationship Id="rId1" Target="worksheets/sheet1.xml"/></Relationships>"#,
                false,
            ),
            (
                "xl/worksheets/sheet1.xml",
                &format!("<worksheet><sheetData>{rows}</sheetData></worksheet>"),
                true,
            ),
        ]);
        let text = quickly(30, move || xlsx_to_text(&book)).unwrap();
        assert!(
            text.contains("\n1\tXFD: 1\n"),
            "{}",
            &text[..200.min(text.len())]
        );
        assert!(text.len() < 2_000_000, "{}", text.len());
    }

    #[test]
    fn a_pictures_size_is_read_from_its_header() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&480u32.to_be_bytes());
        assert_eq!(picture_size(&png), Some((640, 480)));
        let gif = b"GIF89a\x20\x03\x58\x02rest";
        assert_eq!(picture_size(gif), Some((800, 600)));
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0];
        jpeg.extend_from_slice(&[0xff, 0xc0, 0, 17, 8, 0x01, 0x2c, 0x01, 0x90, 3, 0, 0, 0]);
        assert_eq!(picture_size(&jpeg), Some((400, 300)));
        let mut webp = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
        webp.extend_from_slice(&[0x3f, 0x01, 0x00, 0xef, 0x00, 0x00]);
        assert_eq!(picture_size(&webp), Some((320, 240)));
    }

    #[test]
    fn a_window_of_a_long_text_says_how_to_read_on() {
        let text = "abcdefghij";
        assert_eq!(
            window(text, 0, 4),
            "abcd\n[characters 0 to 4 of 10: ask again with from 4 for the rest]"
        );
        assert_eq!(window(text, 8, 4), "[from character 8 of 10]\nij");
        assert!(window(text, 20, 4).contains("nothing from 20"));
        assert_eq!(window(text, 0, 100), text);
    }
}
