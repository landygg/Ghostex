use std::{collections::BTreeMap, fs};

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};

use super::{
    store::{clip_chars, VISUAL_PAGE_MAX_BYTES, VISUAL_PAGE_TITLE_MAX_CHARS},
    theme::{PAGE_BOOTSTRAP, THEME_STYLE},
};

const IMAGE_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// A local path longer than this is not a path a page would carry.
const IMAGE_PATH_MAX_BYTES: usize = 4096;
/// A charset declaration later than this is too late for the browser's prescan.
const CHARSET_SCAN_BYTES: usize = 4 * 1024;
const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "avif", "svg", "bmp", "ico",
];
/// Elements whose content is not markup, so a `<meta>` or `<title>` inside them is not one.
const RAW_TEXT_ELEMENTS: &[&str] = &[
    "script", "style", "textarea", "title", "template", "xmp", "iframe", "noembed", "noframes",
    "noscript",
];

/// CDXC:ServerApi 2026-10-06 WHY:
/// A published page runs its own scripts, and gxserver's default CORS list accepts the `null` origin a sandboxed page sends (`DEFAULT_CORS_ALLOWED_ORIGINS` in config.rs), so without this policy a page could read gxserver's unauthenticated answers on http://127.0.0.1. Every fetch, image, font and media source is limited to https: and inline data, which keeps pages off the loopback listener while CDN libraries still load. serve.rs sends the same policy as a response header, so a page that buries or reorders this tag is still held to it.
pub(super) const PAGE_CONTENT_SECURITY_POLICY: &str = "default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval' https:; style-src 'unsafe-inline' https:; img-src data: blob: https:; font-src data: https:; connect-src https:; media-src data: blob: https:; frame-src 'none'; form-action 'none'; base-uri 'none'";

pub(crate) struct PreparedPage {
    pub(crate) html: String,
    /// The page's own `<title>` text, when it has one.
    pub(crate) title: Option<String>,
}

/// Readies an agent's HTML file for publishing: local images become data URIs, and `<head>`
/// starts with the charset and viewport (when the page has none), the content policy and the
/// Ghostex theme.
pub(crate) fn prepare_page(source: &str) -> Result<PreparedPage, String> {
    let scan = scan_document(source);
    let with_images = inline_local_images(source)?;
    let html = inject_head(&with_images, &scan);
    if html.len() > VISUAL_PAGE_MAX_BYTES {
        return Err(format!(
            "The page with its images is {} MiB; the limit is 8 MiB. Use smaller images.",
            mebibytes(html.len())
        ));
    }
    Ok(PreparedPage {
        html,
        title: scan.title,
    })
}

fn mebibytes(bytes: usize) -> String {
    format!("{:.1}", bytes as f64 / (1024.0 * 1024.0))
}

// ---------------------------------------------------------------------------------------------
// Local images
// ---------------------------------------------------------------------------------------------

struct ImageReference {
    start: usize,
    end: usize,
    path: String,
}

/// CDXC:SessionChat 2026-10-06 WHY:
/// A page opens from gxserver's URL in a browser that cannot read the agent's disk, so a screenshot the agent references by absolute path would show as a broken image. Only files whose bytes really are an image are embedded, so a secret renamed to `.png` is refused instead of carried into a page.
fn inline_local_images(html: &str) -> Result<String, String> {
    let references = find_local_image_references(html);
    if references.is_empty() {
        return Ok(html.to_string());
    }
    let mut images: BTreeMap<&str, Result<(&'static str, Vec<u8>), &'static str>> = BTreeMap::new();
    for reference in &references {
        images
            .entry(reference.path.as_str())
            .or_insert_with(|| load_image(&reference.path));
    }
    let unreadable: Vec<String> = images
        .iter()
        .filter_map(|(path, image)| image.as_ref().err().map(|why| format!("{path} ({why})")))
        .collect();
    if !unreadable.is_empty() {
        return Err(format!(
            "These local images could not be read: {}. Use absolute paths to existing image files, or remove them.",
            unreadable.join(", ")
        ));
    }
    let mut data_uris: BTreeMap<&str, String> = BTreeMap::new();
    let mut total = html.len();
    for reference in &references {
        let Some(Ok((mime, bytes))) = images.get(reference.path.as_str()) else {
            continue;
        };
        total = total - (reference.end - reference.start)
            + "data:;base64,".len()
            + mime.len()
            + bytes.len().div_ceil(3) * 4;
    }
    if total > VISUAL_PAGE_MAX_BYTES {
        return Err(format!(
            "The page with its images would be {} MiB; the limit is 8 MiB. Use smaller images.",
            mebibytes(total)
        ));
    }
    for (path, image) in &images {
        if let Ok((mime, bytes)) = image {
            data_uris.insert(
                *path,
                format!("data:{mime};base64,{}", BASE64_STANDARD.encode(bytes)),
            );
        }
    }
    let mut out = String::with_capacity(total);
    let mut cursor = 0;
    for reference in &references {
        out.push_str(&html[cursor..reference.start]);
        out.push_str(&data_uris[reference.path.as_str()]);
        cursor = reference.end;
    }
    out.push_str(&html[cursor..]);
    Ok(out)
}

/// Absolute image paths that are a whole quoted string (`"…"`, `'…'`, `` `…` ``) or an unquoted
/// CSS `url(…)`, in document order.
fn find_local_image_references(html: &str) -> Vec<ImageReference> {
    let bytes = html.as_bytes();
    let mut references = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if matches!(byte, b'"' | b'\'' | b'`') {
            let start = index + 1;
            if let Some(end) = quoted_path_end(bytes, start, byte) {
                let path = &html[start..end];
                if has_image_extension(path) {
                    references.push(ImageReference {
                        start,
                        end,
                        path: path.to_string(),
                    });
                    index = end + 1;
                    continue;
                }
            }
        } else if bytes[index..]
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"url("))
        {
            let mut start = index + 4;
            while start < bytes.len() && bytes[start].is_ascii_whitespace() {
                start += 1;
            }
            if let Some((end, close)) = unquoted_url_path_end(bytes, start) {
                let path = &html[start..end];
                if has_image_extension(path) {
                    references.push(ImageReference {
                        start,
                        end,
                        path: path.to_string(),
                    });
                    index = close + 1;
                    continue;
                }
            }
        }
        index += 1;
    }
    references
}

/// The closing quote's index when `start` opens an absolute path that runs to it on one line.
fn quoted_path_end(bytes: &[u8], start: usize, quote: u8) -> Option<usize> {
    if !is_absolute_path_start(&bytes[start..]) {
        return None;
    }
    let window = &bytes[start..bytes.len().min(start + IMAGE_PATH_MAX_BYTES)];
    let length = window
        .iter()
        .position(|byte| *byte == quote || matches!(byte, b'\n' | b'\r'))?;
    (window[length] == quote).then_some(start + length)
}

/// `(path end, closing paren)` for an unquoted `url(` argument that is an absolute path.
fn unquoted_url_path_end(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
    if start >= bytes.len() || !is_absolute_path_start(&bytes[start..]) {
        return None;
    }
    let window = &bytes[start..bytes.len().min(start + IMAGE_PATH_MAX_BYTES)];
    let length = window
        .iter()
        .position(|byte| matches!(byte, b')' | b'\n' | b'\r' | b'"' | b'\''))?;
    if window[length] != b')' {
        return None;
    }
    let mut end = start + length;
    while end > start && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    Some((end, start + length))
}

/// POSIX `/…` (not a protocol-relative `//…`) or Windows `C:\…` / `C:/…`.
fn is_absolute_path_start(rest: &[u8]) -> bool {
    match rest {
        [b'/', second, ..] => *second != b'/' && !second.is_ascii_whitespace(),
        [drive, b':', separator, ..] => {
            drive.is_ascii_alphabetic() && matches!(separator, b'\\' | b'/')
        }
        _ => false,
    }
}

fn has_image_extension(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, extension)| {
        IMAGE_EXTENSIONS
            .iter()
            .any(|known| extension.eq_ignore_ascii_case(known))
    })
}

fn load_image(path: &str) -> Result<(&'static str, Vec<u8>), &'static str> {
    let metadata = fs::metadata(path).map_err(|_| "not found")?;
    if !metadata.is_file() {
        return Err("not a file");
    }
    if metadata.len() > IMAGE_MAX_BYTES {
        return Err("larger than 10 MiB");
    }
    let bytes = fs::read(path).map_err(|_| "unreadable")?;
    let mime = sniff_image_mime(&bytes).ok_or("not an image")?;
    Ok((mime, bytes))
}

/// The image type the bytes themselves declare; the file name is never trusted.
fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG") {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF8") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.len() >= 12
        && &bytes[4..8] == b"ftyp"
        && matches!(&bytes[8..12], b"avif" | b"avis" | b"mif1")
    {
        return Some("image/avif");
    }
    // ICO: reserved 0, type 1, then at least one image in the directory.
    if bytes.len() >= 22 && bytes.starts_with(&[0, 0, 1, 0]) && (bytes[4] | bytes[5]) != 0 {
        return Some("image/x-icon");
    }
    // BMP: "BM" plus one of the known DIB header sizes, so a text file that starts with "BM" is not one.
    if bytes.len() >= 26 && bytes.starts_with(b"BM") {
        let header_size = u32::from_le_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]);
        if matches!(header_size, 12 | 40 | 52 | 56 | 64 | 108 | 124) {
            return Some("image/bmp");
        }
    }
    is_svg_document(bytes).then_some("image/svg+xml")
}

/// The root element is `<svg>` once an XML declaration, comments and a doctype are skipped.
fn is_svg_document(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let mut rest = text.strip_prefix('\u{feff}').unwrap_or(text);
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("<?") {
            let Some(end) = after.find("?>") else {
                return false;
            };
            rest = &after[end + 2..];
        } else if let Some(after) = rest.strip_prefix("<!--") {
            let Some(end) = after.find("-->") else {
                return false;
            };
            rest = &after[end + 3..];
        } else if starts_with_ignore_case(rest, "<!doctype") {
            let close = match (rest.find('['), rest.find('>')) {
                (Some(subset), Some(close)) if subset < close => rest.find("]>").map(|end| end + 1),
                (_, close) => close,
            };
            let Some(close) = close else {
                return false;
            };
            rest = &rest[close + 1..];
        } else {
            break;
        }
    }
    rest.strip_prefix("<svg").is_some_and(|after| {
        after.starts_with(|next: char| next.is_ascii_whitespace() || next == '>' || next == '/')
    })
}

// ---------------------------------------------------------------------------------------------
// Document scan and <head> injection
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct DocumentScan {
    has_head: bool,
    has_charset_meta: bool,
    has_viewport_meta: bool,
    title: Option<String>,
}

struct StartTag {
    name: String,
    attributes: Vec<(String, String)>,
    end: usize,
}

impl StartTag {
    fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(attribute, _)| attribute == name)
            .map(|(_, value)| value.as_str())
    }

    fn declares_charset(&self) -> bool {
        self.attribute("charset").is_some()
            || (self
                .attribute("http-equiv")
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("content-type"))
                && self
                    .attribute("content")
                    .is_some_and(|value| value.to_ascii_lowercase().contains("charset=")))
    }
}

/// Walks the start tags that are real markup, skipping comments and the content of raw-text
/// elements (script, style, textarea, template, …), and notes what `inject_head` needs.
fn scan_document(html: &str) -> DocumentScan {
    let mut scan = DocumentScan::default();
    let mut seen_svg = false;
    let mut index = 0;
    while let Some(offset) = html[index..].find('<') {
        let at = index + offset;
        let rest = &html[at..];
        if rest.starts_with("<!--") {
            index = rest[4..]
                .find("-->")
                .map_or(html.len(), |end| at + 4 + end + 3);
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") || rest.starts_with("</") {
            index = rest.find('>').map_or(html.len(), |end| at + end + 1);
            continue;
        }
        let Some(tag) = parse_start_tag(html, at) else {
            index = at + 1;
            continue;
        };
        index = tag.end;
        match tag.name.as_str() {
            "head" => scan.has_head = true,
            "svg" => seen_svg = true,
            "meta" => {
                if at < CHARSET_SCAN_BYTES && tag.declares_charset() {
                    scan.has_charset_meta = true;
                }
                if tag
                    .attribute("name")
                    .is_some_and(|name| name.trim().eq_ignore_ascii_case("viewport"))
                {
                    scan.has_viewport_meta = true;
                }
            }
            _ => {}
        }
        if RAW_TEXT_ELEMENTS.contains(&tag.name.as_str()) {
            let close = find_ignore_case(html, index, &format!("</{}", tag.name));
            let content_end = close.unwrap_or(html.len());
            if tag.name == "title" && !seen_svg && scan.title.is_none() {
                scan.title = clean_title(&html[index..content_end]);
            }
            index = close.map_or(html.len(), |close| {
                html[close..]
                    .find('>')
                    .map_or(html.len(), |end| close + end + 1)
            });
        }
    }
    scan
}

/// A start tag at `at` (which holds `<`), or None when it is not one.
fn parse_start_tag(html: &str, at: usize) -> Option<StartTag> {
    let bytes = html.as_bytes();
    let mut index = at + 1;
    if !bytes.get(index)?.is_ascii_alphabetic() {
        return None;
    }
    let name_start = index;
    while index < bytes.len() && !is_tag_name_end(bytes[index]) {
        index += 1;
    }
    let name = html[name_start..index].to_ascii_lowercase();
    let mut attributes = Vec::new();
    loop {
        while index < bytes.len() && (bytes[index].is_ascii_whitespace() || bytes[index] == b'/') {
            index += 1;
        }
        if *bytes.get(index)? == b'>' {
            return Some(StartTag {
                name,
                attributes,
                end: index + 1,
            });
        }
        let attribute_start = index;
        while index < bytes.len() && !is_tag_name_end(bytes[index]) && bytes[index] != b'=' {
            index += 1;
        }
        let attribute = html[attribute_start..index].to_ascii_lowercase();
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let mut value = String::new();
        if bytes.get(index) == Some(&b'=') {
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            match *bytes.get(index)? {
                quote @ (b'"' | b'\'') => {
                    let value_start = index + 1;
                    let close = value_start
                        + bytes[value_start..]
                            .iter()
                            .position(|byte| *byte == quote)?;
                    value = html[value_start..close].to_string();
                    index = close + 1;
                }
                _ => {
                    let value_start = index;
                    while index < bytes.len()
                        && !bytes[index].is_ascii_whitespace()
                        && bytes[index] != b'>'
                    {
                        index += 1;
                    }
                    value = html[value_start..index].to_string();
                }
            }
        }
        attributes.push((attribute, value));
    }
}

fn is_tag_name_end(byte: u8) -> bool {
    byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>')
}

/// Where the document's markup starts: past a BOM, whitespace, comments, the doctype and the
/// `<html>` start tag, and past `<head>` when that comes next.
struct Prologue {
    bom_end: usize,
    insert_at: usize,
    has_doctype: bool,
    inside_head: bool,
}

fn read_prologue(html: &str) -> Prologue {
    let bom_end = if html.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    let bytes = html.as_bytes();
    let mut prologue = Prologue {
        bom_end,
        insert_at: bom_end,
        has_doctype: false,
        inside_head: false,
    };
    let mut index = bom_end;
    loop {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let rest = &html[index..];
        if rest.starts_with("<!--") {
            let Some(end) = rest[4..].find("-->") else {
                break;
            };
            index += 4 + end + 3;
            continue;
        }
        if starts_with_ignore_case(rest, "<!doctype") || rest.starts_with("<?") {
            let Some(end) = rest.find('>') else {
                break;
            };
            prologue.has_doctype |= rest.starts_with("<!");
            index += end + 1;
            continue;
        }
        if let Some(tag) = parse_start_tag(html, index) {
            if tag.name == "html" {
                index = tag.end;
                continue;
            }
            if tag.name == "head" {
                index = tag.end;
                prologue.inside_head = true;
            }
        }
        break;
    }
    prologue.insert_at = index;
    prologue
}

/// Puts the charset and viewport tags (when missing), the content policy and the theme at the
/// very start of `<head>`, before anything in the page can run or style itself. Without a
/// `<head>` start tag right there, the tags still land in the head: the HTML parser opens one
/// for them, and a later `<head>` tag merges into it.
fn inject_head(html: &str, scan: &DocumentScan) -> String {
    let prologue = read_prologue(html);
    let mut block = String::new();
    if !scan.has_charset_meta {
        block.push_str("<meta charset=\"utf-8\">");
    }
    if !scan.has_viewport_meta {
        block.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    }
    block.push_str("<meta http-equiv=\"Content-Security-Policy\" content=\"");
    block.push_str(PAGE_CONTENT_SECURITY_POLICY);
    block.push_str("\">");
    block.push_str(THEME_STYLE);
    block.push_str(PAGE_BOOTSTRAP);
    if !prologue.inside_head && !scan.has_head {
        block = format!("<head>{block}</head>");
    }
    let mut out = String::with_capacity(html.len() + block.len() + 16);
    if prologue.has_doctype {
        out.push_str(&html[..prologue.insert_at]);
    } else {
        out.push_str(&html[..prologue.bom_end]);
        out.push_str("<!doctype html>");
        out.push_str(&html[prologue.bom_end..prologue.insert_at]);
    }
    out.push_str(&block);
    out.push_str(&html[prologue.insert_at..]);
    out
}

// ---------------------------------------------------------------------------------------------
// Text helpers
// ---------------------------------------------------------------------------------------------

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.len() >= prefix.len()
        && text.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

fn find_ignore_case(text: &str, from: usize, needle: &str) -> Option<usize> {
    let haystack = &text.as_bytes()[from..];
    let needle = needle.as_bytes();
    haystack
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
        .map(|position| from + position)
}

fn clean_title(raw: &str) -> Option<String> {
    let decoded = decode_entities(raw);
    let collapsed = decoded.split_whitespace().collect::<Vec<_>>().join(" ");
    (!collapsed.is_empty()).then(|| clip_chars(&collapsed, VISUAL_PAGE_TITLE_MAX_CHARS))
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(ampersand) = rest.find('&') {
        out.push_str(&rest[..ampersand]);
        rest = &rest[ampersand..];
        let semicolon = rest
            .as_bytes()
            .iter()
            .take(12)
            .position(|byte| *byte == b';');
        let decoded = semicolon.and_then(|semicolon| {
            let entity = &rest[1..semicolon];
            let character = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ => entity.strip_prefix('#').and_then(|number| {
                    let code = match number.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok(),
                        None => number.parse().ok(),
                    };
                    code.and_then(char::from_u32)
                }),
            };
            character.map(|character| (character, semicolon))
        });
        match decoded {
            Some((character, semicolon)) => {
                out.push(character);
                rest = &rest[semicolon + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}
