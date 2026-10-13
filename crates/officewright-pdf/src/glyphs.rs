//! Runs of glyphs at explicit positions, as a layout engine draws the text it has shaped,
//! written as text objects with `TJ` arrays (ISO 32000-1, 9.4).

use std::fmt::Write as _;

use officewright_core::{Color, Pt};
use officewright_font::GlyphId;

use crate::document::{MAX_POSITION, check_position, number};
use crate::embed::{Prepared, Usage, text_string};
use crate::font::check_font_size;
use crate::path::{check_length, check_range, color};
use crate::{Error, Font, Result};

/// The most UTF-16 code units of text a glyph's ToUnicode entry gives; a glyph that stands
/// for longer text is shown with replacement text. A ligature stands for a few characters.
const MAX_MAPPED_UNITS: usize = 32;

/// The most elements an array may have in PDF 1.4 (PDF Reference 1.4, Appendix C), and the
/// most bytes a string may have in the PDF 1.5 that replacement text makes a file: 32767
/// (ISO 32000-1, Annex C; PDF 1.4 allows 65535).
const MAX_ARRAY_ELEMENTS: usize = 8191;
const MAX_STRING_BYTES: usize = 32767;

/// The most UTF-16 code units of text a cluster may have: its replacement text, two bytes
/// a unit after a two-byte byte order mark, must fit in a string.
const MAX_CLUSTER_UNITS: usize = (MAX_STRING_BYTES - 2) / 2;

/// The smallest and largest horizontal scale of a run: 1 % to 1000 %, which covers Word's
/// character scale (`w:w`, 1 % to 600 %).
const HORIZONTAL_SCALE: (f64, f64) = (0.01, 10.0);

/// The largest skew, as a tangent: 45 degrees either way.
const MAX_SKEW: f64 = 1.0;

/// A glyph of a [`GlyphRun`] and where it is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedGlyph {
    glyph: GlyphId,
    x: Pt,
    y: Pt,
}

impl PlacedGlyph {
    /// Glyph `glyph` with its origin `x` to the right of the run's origin and `y` above it,
    /// in points.
    pub const fn new(glyph: GlyphId, x: Pt, y: Pt) -> Self {
        Self { glyph, x, y }
    }

    /// The glyph.
    pub const fn glyph(self) -> GlyphId {
        self.glyph
    }

    /// How far to the right of the run's origin the glyph's origin is.
    pub const fn x(self) -> Pt {
        self.x
    }

    /// How far above the run's origin the glyph's origin is: a mark placed above its base,
    /// for example.
    pub const fn y(self) -> Pt {
        self.y
    }
}

/// Glyphs of an embedded font drawn where a layout engine placed them, with the text they
/// stand for: the way to draw text that has been shaped, with ligatures, kerning, marks
/// and justification. [`Page::draw_glyphs`](crate::Page::draw_glyphs) draws it.
///
/// The run is a sequence of clusters, each some text and the glyphs that show it: one
/// glyph per character, a ligature glyph for several characters, or a base and its marks
/// for one. Text extraction, copying and search read the text of the clusters: the PDF
/// maps each glyph to the text it shows alone, and where a glyph stands for different
/// text in different places, or a cluster has several glyphs or very long text, the
/// cluster gets its own replacement text (`/ActualText`, ISO 32000-1, 14.9.4), which
/// makes the file PDF 1.5.
///
/// The glyphs are drawn in their order, each at its own position: nothing is measured,
/// kerned or shaped here. A run also has a font size, a colour, a horizontal scale that
/// stretches or squeezes its glyphs (Word's character scale), a skew that slants them
/// (synthetic italic), and a stroke around the outlines (synthetic bold).
///
/// One of the drawing primitives the layout engine paints with: public, but exempt from
/// semantic versioning until officewright 1.0 (feature `unstable-drawing`).
///
/// ```no_run
/// use officewright_pdf::{Document, Font, GlyphId, GlyphRun, PageSize, PlacedGlyph, Pt};
///
/// let font = Font::from_file("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 0)?;
/// // A shaper chooses the glyphs and their positions; these are made up.
/// let (ffi, x, acute) = (GlyphId::new(2400), GlyphId::new(91), GlyphId::new(1440));
/// let mut run = GlyphRun::new(&font, Pt(12.0), Pt(72.0), Pt(700.0));
/// // A ligature: one glyph for three characters.
/// run.push_cluster("ffi", [PlacedGlyph::new(ffi, Pt(0.0), Pt(0.0))]);
/// // A base and its mark for one character, the accent raised 1.5 pt.
/// run.push_cluster(
///     "x\u{301}",
///     [
///         PlacedGlyph::new(x, Pt(10.4), Pt(0.0)),
///         PlacedGlyph::new(acute, Pt(11.1), Pt(1.5)),
///     ],
/// );
///
/// let mut doc = Document::new();
/// doc.add_page(PageSize::A4)?.draw_glyphs(&run)?;
/// doc.save("glyphs.pdf")?;
/// # Ok::<(), officewright_pdf::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphRun {
    font: Font,
    size: Pt,
    x: Pt,
    y: Pt,
    color: Color,
    horizontal_scale: f64,
    skew: f64,
    stroke: Option<Pt>,
    glyphs: Vec<PlacedGlyph>,
    /// The text of each cluster and the end of its glyphs in `glyphs`; the cluster's
    /// glyphs start where the previous cluster's end.
    clusters: Vec<(String, usize)>,
}

impl GlyphRun {
    /// An empty run in `font` at `size`, with its origin at (`x`, `y`) on the page: the
    /// point on the baseline that the positions of its glyphs are relative to. Coordinates
    /// are as for [`Page::add_text`](crate::Page::add_text). The run is black, unscaled,
    /// upright and not stroked.
    pub fn new(font: &Font, size: Pt, x: Pt, y: Pt) -> Self {
        Self {
            font: font.clone(),
            size,
            x,
            y,
            color: Color::BLACK,
            horizontal_scale: 1.0,
            skew: 0.0,
            stroke: None,
            glyphs: Vec::new(),
            clusters: Vec::new(),
        }
    }

    /// Sets the colour the glyphs are filled with, and stroked with if they are.
    pub fn with_color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    /// Stretches (above 1) or squeezes (below 1) the glyphs horizontally, from 0.01 to 10;
    /// 1 leaves them as they are. The positions of the glyphs are not scaled: they are
    /// where the layout put the scaled glyphs.
    pub fn with_horizontal_scale(mut self, scale: f64) -> Self {
        self.horizontal_scale = scale;
        self
    }

    /// Slants the run about its baseline by `skew`, the tangent of the angle from the
    /// vertical, from -1 to 1: a point `h` above the baseline moves `h × skew` to the
    /// right, so a positive skew leans the glyphs to the right, as italics do. Glyphs
    /// raised above the baseline, such as marks, move with the slant.
    pub fn with_skew(mut self, skew: f64) -> Self {
        self.skew = skew;
        self
    }

    /// Strokes the outlines of the glyphs with a line `width` points wide, in the run's
    /// colour, as well as filling them: the glyphs look bolder by about the width.
    pub fn with_stroke(mut self, width: Pt) -> Self {
        self.stroke = Some(width);
        self
    }

    /// Appends a cluster: `text` and the glyphs that show it. Text extraction returns the
    /// text, which may be empty for glyphs that stand for no text, such as tab leaders,
    /// and may have up to 16382 UTF-16 code units; a cluster needs at least one glyph.
    pub fn push_cluster(&mut self, text: &str, glyphs: impl IntoIterator<Item = PlacedGlyph>) {
        self.glyphs.extend(glyphs);
        self.clusters.push((text.to_owned(), self.glyphs.len()));
    }

    /// The font.
    pub fn font(&self) -> &Font {
        &self.font
    }

    /// The font size.
    pub fn size(&self) -> Pt {
        self.size
    }

    /// The horizontal position of the origin on the page.
    pub fn x(&self) -> Pt {
        self.x
    }

    /// The vertical position of the origin, the baseline, on the page.
    pub fn y(&self) -> Pt {
        self.y
    }

    /// The colour.
    pub fn color(&self) -> Color {
        self.color
    }

    /// The horizontal scale of the glyphs; 1 for none.
    pub fn horizontal_scale(&self) -> f64 {
        self.horizontal_scale
    }

    /// The skew; 0 for upright glyphs.
    pub fn skew(&self) -> f64 {
        self.skew
    }

    /// The width of the stroke around the outlines, `None` if they are only filled.
    pub fn stroke(&self) -> Option<Pt> {
        self.stroke
    }

    /// The glyphs of all clusters, in the order they are drawn.
    pub fn glyphs(&self) -> &[PlacedGlyph] {
        &self.glyphs
    }

    /// The clusters, each its text and its glyphs, in order.
    pub fn clusters(&self) -> impl Iterator<Item = (&str, &[PlacedGlyph])> {
        let mut start = 0;
        self.clusters.iter().map(move |(text, end)| {
            let glyphs = self.glyphs.get(start..*end).unwrap_or_default();
            start = *end;
            (text.as_str(), glyphs)
        })
    }

    /// Fails unless the run can be drawn: in an embedded font that has all its glyphs,
    /// with every cluster showing a glyph, and every number in range.
    pub(crate) fn check(&self) -> Result<()> {
        let Some(embedded) = self.font.embedded() else {
            return Err(Error::NotEmbedded {
                font: self.font.name().to_owned(),
            });
        };
        check_font_size(self.size)?;
        check_position(self.x, self.y)?;
        let (min, max) = HORIZONTAL_SCALE;
        check_range("horizontal scale", self.horizontal_scale, min, max)?;
        check_range("skew", self.skew, -MAX_SKEW, MAX_SKEW)?;
        if let Some(width) = self.stroke {
            check_length("stroke width", width)?;
        }
        let count = embedded.face.glyph_count();
        for (index, (text, glyphs)) in self.clusters().enumerate() {
            if glyphs.is_empty() {
                return Err(Error::EmptyCluster { cluster: index });
            }
            let units = text.encode_utf16().count();
            if units > MAX_CLUSTER_UNITS {
                return Err(Error::OutOfRange {
                    what: "cluster text length in UTF-16 code units",
                    value: units as f64,
                    min: 0.0,
                    max: MAX_CLUSTER_UNITS as f64,
                });
            }
            for placed in glyphs {
                let glyph = placed.glyph.to_u16();
                if glyph >= count {
                    return Err(Error::InvalidGlyph {
                        glyph,
                        font: embedded.name.clone(),
                    });
                }
                // The vertical offset is written as it is, as the text rise.
                check_position(placed.x, placed.y)?;
                check_position(Pt(self.x.0 + placed.x.0), Pt(self.y.0 + placed.y.0))?;
            }
        }
        Ok(())
    }

    /// Records the glyphs the run shows and the text of those that show a cluster alone.
    pub(crate) fn record(&self, usage: &mut Usage) {
        for (text, glyphs) in self.clusters() {
            match mapped(text, glyphs) {
                Some(glyph) => usage.add(glyph, text),
                None => {
                    for placed in glyphs {
                        usage.add_unmapped(placed.glyph.to_u16());
                    }
                }
            }
        }
    }

    /// Writes the run as a text object that uses the font resource `/F<resource>`, and
    /// returns whether it needed replacement text.
    ///
    /// The run's state is set inside `q` … `Q`, so nothing drawn after it inherits its
    /// colour, scale, rise or rendering mode.
    pub(crate) fn write(
        &self,
        out: &mut Vec<u8>,
        resource: usize,
        prepared: &Prepared<'_>,
    ) -> bool {
        let size = written(self.size.0);
        let scale = written(self.horizontal_scale * 100.0) / 100.0;
        let mut head = format!("q\n{} rg\n", color(self.color));
        if let Some(width) = self.stroke {
            let _ = write!(head, "{} RG\n{} w\n", color(self.color), number(width.0));
        }
        let _ = writeln!(head, "BT\n/F{resource} {} Tf", number(size));
        if scale != 1.0 {
            let _ = writeln!(head, "{} Tz", number(scale * 100.0));
        }
        if self.stroke.is_some() {
            // Fill, then stroke (ISO 32000-1, 9.3.6).
            head.push_str("2 Tr\n");
        }
        out.extend_from_slice(head.as_bytes());

        let mut writer = Writer {
            out,
            run: self,
            prepared,
            // One em of glyph space, 1000 units, is this wide on the page.
            em: size * scale,
            pen: None,
            rise: 0.0,
            array: Vec::new(),
        };
        let mut actual_text = false;
        for (text, glyphs) in self.clusters() {
            let replaced =
                mapped(text, glyphs).is_none_or(|glyph| prepared.usage().text(glyph) != Some(text));
            if replaced {
                writer.end_array();
                let line = format!("/Span << /ActualText {} >> BDC\n", text_string(text));
                writer.out.extend_from_slice(line.as_bytes());
                actual_text = true;
            }
            for &placed in glyphs {
                writer.glyph(placed);
            }
            if replaced {
                writer.end_array();
                writer.out.extend_from_slice(b"EMC\n");
            }
        }
        writer.end_array();
        writer.out.extend_from_slice(b"ET\nQ\n");
        actual_text
    }
}

/// The glyph whose ToUnicode entry can give a cluster's text: its only glyph, if the text
/// is neither empty nor too long for an entry.
fn mapped(text: &str, glyphs: &[PlacedGlyph]) -> Option<u16> {
    match glyphs {
        [placed] if !text.is_empty() && text.encode_utf16().count() <= MAX_MAPPED_UNITS => {
            Some(placed.glyph.to_u16())
        }
        _ => None,
    }
}

/// `value` as a reader reads it back from the file.
fn written(value: f64) -> f64 {
    number(value).parse().unwrap_or(value)
}

/// Writes the glyphs of a run as `TJ` arrays, keeping track of where the reader's text
/// position is so that every glyph lands where the run puts it.
struct Writer<'a, 'b> {
    out: &'a mut Vec<u8>,
    run: &'b GlyphRun,
    prepared: &'b Prepared<'b>,
    /// The width on the page of 1000 units of glyph space: the font size times the
    /// horizontal scale, as written.
    em: f64,
    /// The horizontal text position on the page, as the reader computes it; `None` until
    /// the first glyph sets the text matrix.
    pen: Option<f64>,
    /// The text rise in effect.
    rise: f64,
    /// The elements of the `TJ` array being written: hexadecimal strings of CIDs and
    /// adjustments.
    array: Vec<String>,
}

impl Writer<'_, '_> {
    /// Shows `placed`, moving the text position to it first.
    fn glyph(&mut self, placed: PlacedGlyph) {
        let rise = written(placed.y.0);
        if rise != self.rise {
            self.end_array();
            self.line(&format!("{} Ts", number(rise)));
            self.rise = rise;
        }
        // An array may have 8191 elements in PDF 1.4 (PDF Reference 1.4, Appendix C); this
        // glyph may add two.
        if self.array.len() + 2 > MAX_ARRAY_ELEMENTS {
            self.end_array();
        }
        let x = self.run.x.0 + placed.x.0;
        let pen = match self.pen {
            Some(pen) => {
                // An adjustment moves the next glyph left by its value in thousandths of
                // the em (ISO 32000-1, 9.4.4).
                let adjustment = written((pen - x) * 1000.0 / self.em);
                if adjustment.abs() > MAX_POSITION {
                    // Too far for a real number every reader handles: place the glyph
                    // with a new text matrix instead.
                    self.place(x)
                } else {
                    if adjustment != 0.0 {
                        self.array.push(number(adjustment));
                    }
                    pen - adjustment * self.em / 1000.0
                }
            }
            None => self.place(x),
        };
        let glyph = placed.glyph.to_u16();
        let cid = self.prepared.cid(glyph);
        match self.array.last_mut() {
            // A string may have 32767 bytes, two per CID; the element starts with `<`.
            Some(last) if last.starts_with('<') && last.len() < 1 + 4 * (MAX_STRING_BYTES / 2) => {
                let _ = write!(last, "{cid:04X}");
            }
            _ => self.array.push(format!("<{cid:04X}")),
        }
        self.pen = Some(pen + self.prepared.width(glyph) * self.em / 1000.0);
    }

    /// Sets the text matrix so that the next glyph starts at `x` on the run's baseline,
    /// and returns where that is as the reader computes it.
    fn place(&mut self, x: f64) -> f64 {
        self.end_array();
        self.line(&format!(
            "1 0 {} 1 {} {} Tm",
            number(self.run.skew),
            number(x),
            number(self.run.y.0)
        ));
        written(x)
    }

    /// Writes the `TJ` array, if it has any elements.
    fn end_array(&mut self) {
        if self.array.is_empty() {
            return;
        }
        let elements: Vec<String> = self
            .array
            .drain(..)
            .map(|element| match element.starts_with('<') {
                true => element + ">",
                false => element,
            })
            .collect();
        self.line(&format!("[{}] TJ", elements.join(" ")));
    }

    fn line(&mut self, line: &str) {
        self.out.extend_from_slice(line.as_bytes());
        self.out.push(b'\n');
    }
}
