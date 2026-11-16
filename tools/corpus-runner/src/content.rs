//! What a document holds that the first release does not lay out (plan 1.3, the parts of
//! phase 2 it leaves out, and what phase 1 lays out otherwise and reports), found in its
//! markup. Gate row 11 asks the layout's report of every rendered document to name each
//! kind it holds, so that a user knows why our PDF differs from Word's.
//!
//! Only the stories are read (the body, headers, footers, footnotes and endnotes), not the
//! styles: content a style gives is not found. Deleted text, the formatting a revision
//! replaced and the fallbacks of Markup Compatibility are left out, as the layout leaves
//! them out.

use std::collections::BTreeSet;

use officewright_opc::xml::{Event, Input, NsId, Reader, StartTag};
use officewright_opc::{Limits, Package, Part};

/// The kinds of content, each with the words of which an entry of the layout's report
/// about it names one, in lower case.
pub const KINDS: [(&str, &[&str]); 21] = [
    ("chart", &["chart"]),
    ("SmartArt", &["smartart", "diagram", "drawing"]),
    ("shape", &["shape", "group", "drawing"]),
    ("embedded object", &["embedded object"]),
    ("equation", &["equation"]),
    // Formats other than PNG and JPEG are drawn as same-size placeholders (phase 1).
    ("picture format", &["format", "grey box"]),
    ("columns", &["column"]),
    ("document grid", &["grid"]),
    ("line numbers", &["line number"]),
    ("page borders", &["page border"]),
    // Phase 1 lays out text frames as ordinary paragraphs and floating tables in the text
    // flow, and reports both.
    ("text frame", &["text frame", "drop cap"]),
    ("floating table", &["floating table"]),
    ("vertical alignment", &["vertical alignment"]),
    // An entry may say instead that the object is not drawn at all.
    ("text wrapping", &["wrap", "floating object"]),
    ("vertical text", &["vertical"]),
    ("rotated text box", &["rotate", "rotation", "turned"]),
    ("linked text box", &["linked"]),
    // Tables and floating objects in a text box: basic text boxes hold paragraphs only.
    ("text box content", &["text box"]),
    ("right-to-left text", &["right-to-left", "bidirectional"]),
    ("ruby", &["ruby"]),
    ("text fitting", &["fitting", "fit text"]),
];

/// The words of which an entry about content of `kind` names one; `None` for a kind this
/// runner does not know, from a report of another version.
pub fn words(kind: &str) -> Option<&'static [&'static str]> {
    KINDS
        .iter()
        .find(|(name, _)| *name == kind)
        .map(|(_, words)| *words)
}

/// Whether an entry's message names content of the kind with these words.
pub fn names(message: &str, words: &[&str]) -> bool {
    let message = message.to_lowercase();
    words.iter().any(|word| message.contains(word))
}

/// The kinds of content the Word document at `path` holds, in the order of [`KINDS`].
pub fn scan(path: &std::path::Path) -> Result<Vec<String>, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    scan_bytes(bytes)
}

/// The content types of the stories, without parameters.
const STORIES: [&str; 8] = [
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
    "application/vnd.ms-word.document.macroEnabled.main+xml",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.template.main+xml",
    "application/vnd.ms-word.template.macroEnabledTemplate.main+xml",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml",
];

/// The kinds of content of a package's stories.
pub fn scan_bytes(bytes: Vec<u8>) -> Result<Vec<String>, String> {
    let limits = Limits::new();
    let package = Package::from_bytes(bytes, &limits).map_err(|error| error.to_string())?;
    let mut found = BTreeSet::new();
    for part in package.parts() {
        let story = part
            .content_type()
            .and_then(|kind| kind.split(';').next())
            .is_some_and(|kind| STORIES.contains(&kind.trim()));
        if story {
            scan_part(&part, &limits, &mut found)
                .map_err(|error| format!("{}: {error}", part.name()))?;
        }
    }
    Ok(KINDS
        .iter()
        .filter(|(name, _)| found.contains(name))
        .map(|(name, _)| (*name).to_owned())
        .collect())
}

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const W_STRICT: &str = "http://purl.oclc.org/ooxml/wordprocessingml/main";
const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
const WPS: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingShape";
const WPG: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup";
const WPC: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas";
const V: &str = "urn:schemas-microsoft-com:vml";
const W10: &str = "urn:schemas-microsoft-com:office:word";
const M: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";
const MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const O: &str = "urn:schemas-microsoft-com:office:office";
const CHARTS: [&str; 2] = [
    "http://schemas.openxmlformats.org/drawingml/2006/chart",
    "http://schemas.microsoft.com/office/drawing/2014/chartex",
];
const DIAGRAM: &str = "http://schemas.openxmlformats.org/drawingml/2006/diagram";

/// The VML elements that draw a shape.
const VML_SHAPES: [&str; 8] = [
    "shape",
    "rect",
    "roundrect",
    "oval",
    "line",
    "polyline",
    "arc",
    "curve",
];

/// The picture formats the layout draws.
const DRAWN: [&str; 5] = [
    "image/png",
    "image/x-png",
    "image/jpeg",
    "image/jpg",
    "image/pjpeg",
];

/// The namespaces a part's names are compared with.
struct Names {
    w: [NsId; 2],
    a: NsId,
    wp: NsId,
    wps: NsId,
    wpg: NsId,
    wpc: NsId,
    v: NsId,
    w10: NsId,
    m: NsId,
    mc: NsId,
    r: NsId,
    o: NsId,
}

impl Names {
    fn new(reader: &mut Reader) -> Self {
        Names {
            w: [reader.intern(W), reader.intern(W_STRICT)],
            a: reader.intern(A),
            wp: reader.intern(WP),
            wps: reader.intern(WPS),
            wpg: reader.intern(WPG),
            wpc: reader.intern(WPC),
            v: reader.intern(V),
            w10: reader.intern(W10),
            m: reader.intern(M),
            mc: reader.intern(MC),
            r: reader.intern(R),
            o: reader.intern(O),
        }
    }
}

/// An open element the scan keeps: what the kinds that depend on its content need.
enum Open {
    /// A DrawingML shape: whether it holds a text box, and whether it is turned.
    Shape { text: bool, turned: bool },
    /// A VML shape outside an embedded object: whether it holds a text box or a picture,
    /// and whether its style turns it.
    Vml {
        text: bool,
        picture: bool,
        turned: bool,
        /// Its `id` and `o:spid`, which a box that is in no chain names as its next.
        names: [Option<String>; 2],
    },
    /// An embedded object: whether it shows a VML preview picture or a drawing.
    Object { preview: bool, drawing: bool },
    /// The paragraphs of a text box.
    TextBox,
    /// A section's properties.
    Section,
    /// The columns of a section, with the w:col elements counted.
    Columns(usize),
    /// A section's page borders.
    PageBorders,
    /// Anything else.
    Other,
}

fn scan_part(
    part: &Part,
    limits: &Limits,
    found: &mut BTreeSet<&'static str>,
) -> Result<(), String> {
    let bytes = part.read().map_err(|error| error.to_string())?;
    let input = Input::decode(&bytes).map_err(|error| error.to_string())?;
    let mut reader = input.reader(limits);
    let names = Names::new(&mut reader);
    let mut open: Vec<Open> = Vec::new();
    while let Some(event) = reader.read_event().map_err(|error| error.to_string())? {
        match event {
            Event::Start(tag) => {
                if skipped(&tag, &names) {
                    reader.skip_element().map_err(|error| error.to_string())?;
                    continue;
                }
                let element = start(&tag, &names, part, &mut open, found);
                open.push(element);
            }
            Event::End(_) => {
                if let Some(element) = open.pop() {
                    end(element, found);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Whether the layout leaves an element out with all it holds: deleted text, the
/// formatting a revision replaced (w:pPrChange and the like), the fallbacks of Markup
/// Compatibility, and VML shape types, which only define shapes.
fn skipped(tag: &StartTag, names: &Names) -> bool {
    let name = tag.name();
    let local = name.local();
    if names.w.iter().any(|w| name.namespace() == Some(*w)) {
        matches!(local, "del" | "moveFrom") || local.ends_with("Change")
    } else {
        name.is(names.mc, "Fallback") || name.is(names.v, "shapetype")
    }
}

/// Whether an on/off attribute is on: absent, or not 0, false or off.
fn on(value: Option<&str>) -> bool {
    !matches!(value, Some("0" | "false" | "off"))
}

/// The value of attribute `local` of `tag` in WordprocessingML's namespace.
fn w_value<'t>(tag: &'t StartTag, names: &Names, local: &str) -> Option<&'t str> {
    names.w.iter().find_map(|w| tag.attribute(Some(*w), local))
}

/// The value of a property of a VML style (`rotation:90;mso-next-textbox:#t2`).
fn style_property<'s>(style: &'s str, property: &str) -> Option<&'s str> {
    style.split(';').find_map(|declaration| {
        let (name, value) = declaration.split_once(':')?;
        (name.trim().eq_ignore_ascii_case(property)).then(|| value.trim())
    })
}

/// Whether a rotation, in DrawingML's units of 1/60,000 degree or VML's degrees (a number
/// with an optional unit such as `fd`), turns its object.
fn turns(value: &str) -> bool {
    let number: String = value
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.'))
        .collect();
    number.parse::<f64>().is_ok_and(|turn| turn != 0.0)
}

/// Whether the part a relationship id names is a picture in a format the layout does not
/// draw.
fn undrawn_picture(part: &Part, id: &str) -> bool {
    let Some(target) = part
        .relationships()
        .get(id)
        .and_then(|rel| rel.target_part())
    else {
        return false;
    };
    let Some(kind) = part
        .package()
        .part(&target)
        .and_then(|picture| picture.content_type())
    else {
        return false;
    };
    let kind = kind.split(';').next().unwrap_or_default().trim();
    kind.starts_with("image/") && !DRAWN.contains(&kind.to_ascii_lowercase().as_str())
}

/// What a start tag shows, and the element to keep open for it.
fn start(
    tag: &StartTag,
    names: &Names,
    part: &Part,
    open: &mut [Open],
    found: &mut BTreeSet<&'static str>,
) -> Open {
    let name = tag.name();
    let local = name.local();
    let in_object = open
        .iter()
        .any(|element| matches!(element, Open::Object { .. }));
    let in_text_box = open.iter().any(|element| matches!(element, Open::TextBox));
    if names.w.iter().any(|w| name.namespace() == Some(*w)) {
        let parent_section = matches!(open.last(), Some(Open::Section));
        match local {
            "sectPr" => return Open::Section,
            "cols" if parent_section => {
                let num = w_value(tag, names, "num").and_then(|num| num.parse::<u32>().ok());
                if num.is_some_and(|num| num > 1) {
                    found.insert("columns");
                }
                return Open::Columns(0);
            }
            "col" => {
                if let Some(Open::Columns(count)) = open.last_mut() {
                    *count += 1;
                }
            }
            "docGrid" if parent_section => {
                if matches!(
                    w_value(tag, names, "type"),
                    Some("lines" | "linesAndChars" | "snapToChars")
                ) {
                    found.insert("document grid");
                }
            }
            "lnNumType" if parent_section => {
                if w_value(tag, names, "countBy") != Some("0") {
                    found.insert("line numbers");
                }
            }
            "pgBorders" if parent_section => return Open::PageBorders,
            "top" | "left" | "bottom" | "right" | "start" | "end"
                if matches!(open.last(), Some(Open::PageBorders)) =>
            {
                if !matches!(w_value(tag, names, "val"), Some("nil" | "none")) {
                    found.insert("page borders");
                }
            }
            "vAlign" if parent_section => {
                if matches!(
                    w_value(tag, names, "val"),
                    Some("center" | "both" | "bottom")
                ) {
                    found.insert("vertical alignment");
                }
            }
            "framePr" => {
                found.insert("text frame");
            }
            "tblpPr" => {
                found.insert("floating table");
            }
            "textDirection" => {
                if !matches!(w_value(tag, names, "val"), Some("lrTb" | "lr") | None) {
                    found.insert("vertical text");
                }
            }
            "bidi" | "rtl" | "bidiVisual" => {
                if on(w_value(tag, names, "val")) {
                    found.insert("right-to-left text");
                }
            }
            "ruby" => {
                found.insert("ruby");
            }
            "fitText" => {
                found.insert("text fitting");
            }
            "tcFitText" => {
                if on(w_value(tag, names, "val")) {
                    found.insert("text fitting");
                }
            }
            "object" => {
                return Open::Object {
                    preview: false,
                    drawing: false,
                };
            }
            "drawing" => {
                if let Some(Open::Object { drawing, .. }) = open
                    .iter_mut()
                    .rev()
                    .find(|e| matches!(e, Open::Object { .. }))
                {
                    *drawing = true;
                }
            }
            "txbxContent" => return Open::TextBox,
            "tbl" if in_text_box => {
                found.insert("text box content");
            }
            _ => {}
        }
    } else if name.namespace() == Some(names.a) {
        match local {
            "graphicData" => match tag.attribute(None, "uri") {
                Some(uri) if CHARTS.contains(&uri) => {
                    found.insert("chart");
                }
                Some(DIAGRAM) => {
                    found.insert("SmartArt");
                }
                _ => {}
            },
            "blip" => {
                if tag
                    .attribute(Some(names.r), "embed")
                    .is_some_and(|id| undrawn_picture(part, id))
                {
                    found.insert("picture format");
                }
            }
            "xfrm" if tag.attribute(None, "rot").is_some_and(turns) => {
                if let Some(Open::Shape { turned, .. }) = open
                    .iter_mut()
                    .rev()
                    .find(|e| matches!(e, Open::Shape { .. }))
                {
                    *turned = true;
                }
            }
            _ => {}
        }
    } else if name.namespace() == Some(names.wps) {
        match local {
            "wsp" => {
                return Open::Shape {
                    text: false,
                    turned: false,
                };
            }
            "txbx" | "linkedTxbx" => {
                if local == "linkedTxbx" {
                    found.insert("linked text box");
                }
                if let Some(Open::Shape { text, .. }) = open
                    .iter_mut()
                    .rev()
                    .find(|e| matches!(e, Open::Shape { .. }))
                {
                    *text = true;
                }
            }
            "bodyPr" if !matches!(tag.attribute(None, "vert"), Some("horz") | None) => {
                found.insert("vertical text");
            }
            _ => {}
        }
    } else if name.is(names.wpg, "wgp") || name.is(names.wpc, "wpc") {
        found.insert("shape");
    } else if name.namespace() == Some(names.wp) {
        match local {
            "wrapSquare" | "wrapTight" | "wrapThrough" | "wrapTopAndBottom" => {
                found.insert("text wrapping");
            }
            "anchor" if in_text_box => {
                found.insert("text box content");
            }
            _ => {}
        }
    } else if name.is(names.w10, "wrap") {
        if matches!(
            tag.attribute(None, "type"),
            Some("square" | "tight" | "through" | "topAndBottom")
        ) {
            found.insert("text wrapping");
        }
    } else if name.namespace() == Some(names.m) {
        if matches!(local, "oMath" | "oMathPara") {
            found.insert("equation");
        }
    } else if name.namespace() == Some(names.v) {
        if local == "group" {
            found.insert("shape");
        } else if VML_SHAPES.contains(&local) {
            if in_object {
                return Open::Other;
            }
            let style = tag.attribute(None, "style").unwrap_or_default();
            return Open::Vml {
                text: false,
                picture: false,
                turned: style_property(style, "rotation").is_some_and(turns),
                names: [
                    tag.attribute(None, "id").map(str::to_owned),
                    tag.attribute(Some(names.o), "spid").map(str::to_owned),
                ],
            };
        } else if local == "textbox" {
            let style = tag.attribute(None, "style").unwrap_or_default();
            // A box that names itself as the next is in no chain, as Word writes some
            // (business/report-hhs-usability-test-report.docx); officewright-doc reads it so.
            let shape = open.iter().rev().find_map(|element| match element {
                Open::Vml { names, .. } => Some(names),
                _ => None,
            });
            let next = style_property(style, "mso-next-textbox")
                .map(|next| next.strip_prefix('#').unwrap_or(next))
                .filter(|next| !next.is_empty());
            if next.is_some_and(|next| {
                !shape.is_some_and(|names| names.iter().flatten().any(|name| name == next))
            }) {
                found.insert("linked text box");
            }
            if style_property(style, "layout-flow").is_some_and(|flow| flow.starts_with("vertical"))
            {
                found.insert("vertical text");
            }
            if let Some(Open::Vml { text, .. }) = open
                .iter_mut()
                .rev()
                .find(|e| matches!(e, Open::Vml { .. }))
            {
                *text = true;
            }
        } else if local == "imagedata" {
            if tag
                .attribute(Some(names.r), "id")
                .is_some_and(|id| undrawn_picture(part, id))
            {
                found.insert("picture format");
            }
            for element in open.iter_mut().rev() {
                match element {
                    Open::Vml { picture, .. } => {
                        *picture = true;
                        break;
                    }
                    Open::Object { preview, .. } => {
                        *preview = true;
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
    Open::Other
}

/// What an element shows once its content is read.
fn end(element: Open, found: &mut BTreeSet<&'static str>) {
    match element {
        Open::Shape { text, turned } => {
            if !text {
                found.insert("shape");
            } else if turned {
                found.insert("rotated text box");
            }
        }
        Open::Vml {
            text,
            picture,
            turned,
            ..
        } => {
            if text && turned {
                found.insert("rotated text box");
            } else if !text && !picture {
                found.insert("shape");
            }
        }
        Open::Object { preview, drawing } => {
            if drawing || !preview {
                found.insert("embedded object");
            }
        }
        Open::Columns(count) if count > 1 => {
            found.insert("columns");
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// A ZIP archive of `files`, stored without compression.
    fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in files {
            let offset = out.len() as u32;
            let (crc, size, length) = (crc32(data), data.len() as u32, name.len() as u16);
            let header = |signature: u32, central: bool| {
                let mut h = signature.to_le_bytes().to_vec();
                if central {
                    h.extend(20u16.to_le_bytes());
                }
                h.extend(20u16.to_le_bytes());
                h.extend([0u8; 6]); // flags, method (stored), time
                h.extend(0x21u16.to_le_bytes()); // date
                h.extend(crc.to_le_bytes());
                h.extend(size.to_le_bytes());
                h.extend(size.to_le_bytes());
                h.extend(length.to_le_bytes());
                h.extend(0u16.to_le_bytes());
                if central {
                    h.extend([0u8; 6]); // comment, disk, internal attributes
                    h.extend(0u32.to_le_bytes());
                    h.extend(offset.to_le_bytes());
                }
                h.extend(name.as_bytes());
                h
            };
            out.extend(header(0x0403_4b50, false));
            out.extend(*data);
            central.extend(header(0x0201_4b50, true));
        }
        let (start, count) = (out.len() as u32, files.len() as u16);
        let size = central.len() as u32;
        out.extend(central);
        out.extend(0x0605_4b50u32.to_le_bytes());
        out.extend([0u8; 4]);
        out.extend(count.to_le_bytes());
        out.extend(count.to_le_bytes());
        out.extend(size.to_le_bytes());
        out.extend(start.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out
    }

    const NAMESPACES: &str = concat!(
        r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" "#,
        r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" "#,
        r#"xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" "#,
        r#"xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" "#,
        r#"xmlns:wpg="http://schemas.microsoft.com/office/word/2010/wordprocessingGroup" "#,
        r#"xmlns:v="urn:schemas-microsoft-com:vml" "#,
        r#"xmlns:w10="urn:schemas-microsoft-com:office:word" "#,
        r#"xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math" "#,
        r#"xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" "#,
        r#"xmlns:o="urn:schemas-microsoft-com:office:office" "#,
        r#"xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#,
    );

    /// A Word document whose body is `body`, with a header `header`, a styles part
    /// `styles` and pictures rId1 (EMF) and rId2 (PNG).
    fn document(body: &str, header: &str, styles: &str) -> Vec<u8> {
        let main = "application/vnd.openxmlformats-officedocument.wordprocessingml";
        let types = format!(
            r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Default Extension="emf" ContentType="image/x-emf"/>
<Default Extension="png" ContentType="image/png"/>
<Override PartName="/word/document.xml" ContentType="{main}.document.main+xml"/>
<Override PartName="/word/header1.xml" ContentType="{main}.header+xml"/>
<Override PartName="/word/styles.xml" ContentType="{main}.styles+xml"/></Types>"#
        );
        let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
        let image = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
        let document_rels = format!(
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{image}" Target="media/image1.emf"/><Relationship Id="rId2" Type="{image}" Target="media/image2.png"/></Relationships>"#
        );
        let story = |root: &str, content: &str| {
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><w:{root} {NAMESPACES}>{content}</w:{root}>"#
            )
        };
        let body = story("document", &format!("<w:body>{body}</w:body>"));
        let header = story("hdr", header);
        let styles = story("styles", styles);
        zip(&[
            ("[Content_Types].xml", types.as_bytes()),
            ("_rels/.rels", rels.as_bytes()),
            ("word/document.xml", body.as_bytes()),
            ("word/_rels/document.xml.rels", document_rels.as_bytes()),
            ("word/header1.xml", header.as_bytes()),
            ("word/styles.xml", styles.as_bytes()),
            ("word/media/image1.emf", b"EMF"),
            ("word/media/image2.png", b"PNG"),
        ])
    }

    fn kinds(body: &str) -> Vec<String> {
        scan_bytes(document(body, "", "")).unwrap()
    }

    fn graphic(uri: &str, content: &str) -> String {
        format!(
            r#"<w:p><w:r><w:drawing><wp:inline><a:graphic><a:graphicData uri="{uri}">{content}</a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#
        )
    }

    fn anchored(wrap: &str) -> String {
        format!(
            r#"<w:p><w:r><w:drawing><wp:anchor>{wrap}<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><a:blip r:embed="rId2"/></a:graphicData></a:graphic></wp:anchor></w:drawing></w:r></w:p>"#
        )
    }

    fn shape(content: &str) -> String {
        graphic(
            "http://schemas.microsoft.com/office/word/2010/wordprocessingShape",
            &format!("<wps:wsp>{content}</wps:wsp>"),
        )
    }

    const TEXT: &str = "<wps:txbx><w:txbxContent><w:p/></w:txbxContent></wps:txbx>";

    fn section(content: &str) -> String {
        format!("<w:p/><w:sectPr>{content}</w:sectPr>")
    }

    fn pict(content: &str) -> String {
        format!("<w:p><w:r><w:pict>{content}</w:pict></w:r></w:p>")
    }

    /// Each kind, from the smallest markup that holds it.
    #[test]
    fn every_kind_is_found() {
        let cases: Vec<(String, &str)> = vec![
            (graphic("http://schemas.openxmlformats.org/drawingml/2006/chart", ""), "chart"),
            (
                graphic("http://schemas.microsoft.com/office/drawing/2014/chartex", ""),
                "chart",
            ),
            (
                graphic("http://schemas.openxmlformats.org/drawingml/2006/diagram", ""),
                "SmartArt",
            ),
            (shape("<wps:spPr/>"), "shape"),
            (
                graphic(
                    "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup",
                    "<wpg:wgp/>",
                ),
                "shape",
            ),
            (pict(r#"<v:rect style="width:10pt"/>"#), "shape"),
            (pict(r#"<v:line from="0,0" to="10,0"/>"#), "shape"),
            (pict("<v:group><v:shape/></v:group>"), "shape"),
            ("<w:p><w:r><w:object/></w:r></w:p>".to_owned(), "embedded object"),
            (
                "<w:p><w:r><w:object><w:drawing/></w:object></w:r></w:p>".to_owned(),
                "embedded object",
            ),
            ("<w:p><m:oMathPara><m:oMath/></m:oMathPara></w:p>".to_owned(), "equation"),
            (
                graphic(
                    "http://schemas.openxmlformats.org/drawingml/2006/picture",
                    r#"<a:blip r:embed="rId1"/>"#,
                ),
                "picture format",
            ),
            (
                pict(r#"<v:shape><v:imagedata r:id="rId1"/></v:shape>"#),
                "picture format",
            ),
            (section(r#"<w:cols w:num="2"/>"#), "columns"),
            (
                section(r#"<w:cols w:equalWidth="0"><w:col w:w="10"/><w:col w:w="10"/></w:cols>"#),
                "columns",
            ),
            (section(r#"<w:docGrid w:type="lines" w:linePitch="360"/>"#), "document grid"),
            (section(r#"<w:lnNumType w:countBy="1"/>"#), "line numbers"),
            (
                section(r#"<w:pgBorders><w:top w:val="single"/></w:pgBorders>"#),
                "page borders",
            ),
            (section(r#"<w:vAlign w:val="center"/>"#), "vertical alignment"),
            (r#"<w:p><w:pPr><w:framePr w:dropCap="drop"/></w:pPr></w:p>"#.to_owned(), "text frame"),
            (
                r#"<w:tbl><w:tblPr><w:tblpPr w:tblpY="10"/></w:tblPr></w:tbl>"#.to_owned(),
                "floating table",
            ),
            (anchored("<wp:wrapSquare/>"), "text wrapping"),
            (anchored("<wp:wrapTopAndBottom/>"), "text wrapping"),
            (pict(r#"<v:rect><v:textbox/><w10:wrap type="square"/></v:rect>"#), "text wrapping"),
            (
                r#"<w:tbl><w:tr><w:tc><w:tcPr><w:textDirection w:val="btLr"/></w:tcPr></w:tc></w:tr></w:tbl>"#
                    .to_owned(),
                "vertical text",
            ),
            (shape(&format!(r#"<wps:bodyPr vert="vert270"/>{TEXT}"#)), "vertical text"),
            (
                pict(r#"<v:rect><v:textbox style="layout-flow:vertical"/></v:rect>"#),
                "vertical text",
            ),
            (
                shape(&format!(r#"<wps:spPr><a:xfrm rot="5400000"/></wps:spPr>{TEXT}"#)),
                "rotated text box",
            ),
            (
                pict(r#"<v:rect style="rotation:90"><v:textbox/></v:rect>"#),
                "rotated text box",
            ),
            (shape(r#"<wps:linkedTxbx id="1" seq="1"/>"#), "linked text box"),
            (
                pict(
                    r##"<v:rect id="t1" o:spid="s1"><v:textbox style="mso-next-textbox:#t2"/></v:rect>"##,
                ),
                "linked text box",
            ),
            (
                shape("<wps:txbx><w:txbxContent><w:tbl/></w:txbxContent></wps:txbx>"),
                "text box content",
            ),
            (
                shape(&format!(
                    "<wps:txbx><w:txbxContent>{}</w:txbxContent></wps:txbx>",
                    anchored("")
                )),
                "text box content",
            ),
            (r#"<w:p><w:pPr><w:bidi/></w:pPr></w:p>"#.to_owned(), "right-to-left text"),
            (
                r#"<w:p><w:r><w:rPr><w:rtl w:val="1"/></w:rPr></w:r></w:p>"#.to_owned(),
                "right-to-left text",
            ),
            (r#"<w:p><w:r><w:ruby/></w:r></w:p>"#.to_owned(), "ruby"),
            (
                r#"<w:p><w:r><w:rPr><w:fitText w:val="720"/></w:rPr></w:r></w:p>"#.to_owned(),
                "text fitting",
            ),
            (
                r#"<w:tbl><w:tr><w:tc><w:tcPr><w:tcFitText/></w:tcPr></w:tc></w:tr></w:tbl>"#
                    .to_owned(),
                "text fitting",
            ),
        ];
        let mut seen = BTreeSet::new();
        for (body, kind) in &cases {
            assert_eq!(kinds(body), [*kind], "{body}");
            seen.insert(*kind);
        }
        let all: BTreeSet<&str> = KINDS.iter().map(|(name, _)| *name).collect();
        assert_eq!(seen, all, "every kind has a case");
    }

    /// What the layout lays out, or leaves out as Word does, is no such content.
    #[test]
    fn what_the_layout_lays_out_is_not_found() {
        let cases: Vec<String> = vec![
            // A picture in PNG, inline and anchored without wrapping; a basic text box.
            graphic(
                "http://schemas.openxmlformats.org/drawingml/2006/picture",
                r#"<a:blip r:embed="rId2"/>"#,
            ),
            anchored("<wp:wrapNone/>"),
            shape(TEXT),
            shape(&format!(r#"<wps:bodyPr vert="horz"/><wps:spPr><a:xfrm rot="0"/></wps:spPr>{TEXT}"#)),
            pict(r#"<v:rect style="rotation:0"><v:textbox style="layout-flow:horizontal"/></v:rect>"#),
            pict(r#"<v:shape><v:imagedata r:id="rId2"/></v:shape>"#),
            // A turned picture is not a turned text box; wrapping in front of the text.
            pict(r#"<v:shape style="rotation:90"><v:imagedata r:id="rId2"/><w10:wrap type="none"/></v:shape>"#),
            // An embedded object shown by its preview picture; a shape type defines only.
            r#"<w:p><w:r><w:object><v:shapetype/><v:shape><v:imagedata r:id="rId2"/></v:shape></w:object></w:r></w:p>"#.to_owned(),
            pict("<v:shapetype><v:path/></v:shapetype>"),
            // A box that names itself as its next one, by its id or its o:spid.
            pict(r##"<v:rect id="t1"><v:textbox style="mso-next-textbox:#t1"/></v:rect>"##),
            pict(
                r##"<v:rect o:spid="_x0000_s2050"><v:textbox style="mso-next-textbox:#_x0000_s2050"/></v:rect>"##,
            ),
            pict(r##"<v:rect><v:textbox style="mso-next-textbox:"/></v:rect>"##),
            // One column, the default grid and line numbering off, page borders of none.
            section(r#"<w:cols w:num="1" w:space="720"/><w:docGrid w:type="default"/><w:lnNumType w:countBy="0"/><w:pgBorders><w:top w:val="none"/><w:left w:val="nil"/></w:pgBorders><w:vAlign w:val="top"/>"#),
            section(r#"<w:cols><w:col w:w="10"/></w:cols>"#),
            // A cell's vertical alignment; text from left to right.
            r#"<w:tbl><w:tr><w:tc><w:tcPr><w:vAlign w:val="center"/><w:textDirection w:val="lrTb"/><w:tcFitText w:val="0"/></w:tcPr></w:tc></w:tr></w:tbl>"#.to_owned(),
            r#"<w:p><w:pPr><w:bidi w:val="0"/></w:pPr><w:r><w:rPr><w:rtl w:val="false"/></w:rPr></w:r></w:p>"#.to_owned(),
            // Deleted content, the formatting a revision replaced, a fallback.
            format!("<w:p><w:del>{}</w:del></w:p>", "<w:r><w:object/></w:r>"),
            r#"<w:p><w:pPr><w:pPrChange><w:pPr><w:framePr w:w="10"/></w:pPr></w:pPrChange></w:pPr></w:p>"#.to_owned(),
            section(r#"<w:sectPrChange><w:sectPr><w:cols w:num="2"/></w:sectPr></w:sectPrChange>"#),
            format!(
                "<w:p><w:r><mc:AlternateContent><mc:Choice Requires=\"wps\">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent></w:r></w:p>",
                "<w:drawing/>",
                "<w:pict><v:rect/></w:pict>"
            ),
        ];
        for body in &cases {
            assert_eq!(kinds(body), Vec::<String>::new(), "{body}");
        }
    }

    /// The stories are read, headers too, and the styles are not.
    #[test]
    fn headers_are_read_and_styles_are_not() {
        let chart = graphic("http://schemas.openxmlformats.org/drawingml/2006/chart", "");
        assert_eq!(
            scan_bytes(document("<w:p/>", &chart, "")).unwrap(),
            ["chart"]
        );
        let framed = r#"<w:style w:styleId="Frame"><w:pPr><w:framePr w:w="10"/></w:pPr></w:style>"#;
        assert!(
            scan_bytes(document("<w:p/>", "", framed))
                .unwrap()
                .is_empty()
        );
        // Kinds come in the order of KINDS, each once.
        let both = format!("{}{}{}", section(r#"<w:cols w:num="3"/>"#), chart, chart);
        assert_eq!(kinds(&both), ["chart", "columns"]);
    }

    #[test]
    fn damaged_markup_is_an_error() {
        let error = scan_bytes(document("<w:p>", "", "")).unwrap_err();
        assert!(error.starts_with("/word/document.xml: "), "{error}");
        assert!(scan_bytes(b"not a zip".to_vec()).is_err());
        assert!(scan(std::path::Path::new("/nonexistent/a.docx")).is_err());
    }

    #[test]
    fn entries_name_content_by_their_words() {
        let wrapping = words("text wrapping").unwrap();
        assert!(names(
            "Text does not WRAP around floating objects yet",
            wrapping
        ));
        assert!(names(
            "floating objects in table cells are not drawn yet",
            wrapping
        ));
        assert!(!names("paragraph borders are not drawn yet", wrapping));
        let frames = words("text frame").unwrap();
        assert!(names(
            "a drop cap (w:framePr) is laid out as an ordinary paragraph",
            frames
        ));
        assert!(names(
            "a text frame (w:framePr) is laid out as an ordinary paragraph",
            frames
        ));
        assert_eq!(words("no such kind"), None);
        for (name, words) in KINDS {
            assert!(!words.is_empty(), "{name}");
            for word in words {
                assert_eq!(*word, word.to_lowercase(), "{name}");
            }
        }
    }
}
