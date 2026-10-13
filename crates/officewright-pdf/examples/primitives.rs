//! Writes a one-page PDF with glyph runs and vector graphics in a TrueType or OpenType
//! font, to look at by hand:
//!
//! ```text
//! cargo run -p officewright-pdf --example primitives -- FONT OUT.pdf
//! ```
//!
//! The glyphs are chosen with the font's character map and placed by their advance
//! widths: a small stand-in for the layout engine, which shapes text. The page shows
//! ligatures, letter spacing, a horizontally scaled run, a slanted one, a stroked one, an
//! ellipsis drawn as three full stops, small capitals, a raised digit, a highlight, a
//! dashed frame, a dotted line, a double line, a wavy line, a triangle, a clipped circle
//! and two nested clips. Text extraction must return every text as written.
//! tests/pdf-output/test_primitives.py checks the page at the positions given here.

use std::error::Error;

use officewright_font::FontFace;
use officewright_pdf::{
    Color, Document, Font, GlyphId, GlyphRun, GraphicsPath, LineCap, PageSize, PlacedGlyph, Pt,
    Stroke,
};

/// The ligatures tried, longest first, with the presentation form a font maps to the
/// ligature glyph.
const LIGATURES: [(&str, char); 5] = [
    ("ffi", '\u{FB03}'),
    ("ffl", '\u{FB04}'),
    ("ff", '\u{FB00}'),
    ("fi", '\u{FB01}'),
    ("fl", '\u{FB02}'),
];

/// How [`Writer::run`] chooses and places glyphs.
struct Style<'a> {
    /// Points added after every glyph.
    spacing: f64,
    /// The horizontal scale of the glyphs, which their advances follow.
    scale: f64,
    /// Characters raised by `rise` points.
    raised: &'a str,
    rise: f64,
    /// Letters are drawn with the glyphs of their capitals, as small capitals are.
    capitals: bool,
}

const PLAIN: Style<'static> = Style {
    spacing: 0.0,
    scale: 1.0,
    raised: "",
    rise: 0.0,
    capitals: false,
};

/// Glyph runs made with a font's character map.
struct Writer<'a> {
    face: &'a FontFace,
    font: &'a Font,
}

impl Writer<'_> {
    /// A run of `text` at `size` with its origin at (`x`, `y`): one glyph per character,
    /// a ligature glyph for the characters of a ligature the font has, and three full
    /// stops for an ellipsis, each glyph where the one before ends.
    fn run(
        &self,
        text: &str,
        size: f64,
        x: f64,
        y: f64,
        style: &Style<'_>,
    ) -> Result<GlyphRun, Box<dyn Error>> {
        let map = self.face.char_map();
        let glyph = |ch: char| map.map(ch).ok_or_else(|| format!("the font has no {ch:?}"));
        let mut run =
            GlyphRun::new(self.font, Pt(size), Pt(x), Pt(y)).with_horizontal_scale(style.scale);
        let mut pen = 0.0;
        let mut rest = text;
        while let Some(ch) = rest.chars().next() {
            let ligature = LIGATURES
                .iter()
                .find(|(chars, form)| rest.starts_with(chars) && map.map(*form).is_some());
            let (cluster, glyphs) = match (ligature, ch) {
                (Some(&(chars, form)), _) => (chars, vec![glyph(form)?]),
                (None, '…') => ("…", vec![glyph('.')?; 3]),
                (None, _) if style.capitals => {
                    let capital = ch.to_uppercase().next().unwrap_or(ch);
                    (&rest[..ch.len_utf8()], vec![glyph(capital)?])
                }
                (None, _) => (&rest[..ch.len_utf8()], vec![glyph(ch)?]),
            };
            let rise = if style.raised.contains(ch) {
                style.rise
            } else {
                0.0
            };
            let mut placed = Vec::new();
            for glyph in glyphs {
                placed.push(PlacedGlyph::new(glyph, Pt(pen), Pt(rise)));
                pen += self.advance(&run, glyph);
            }
            run.push_cluster(cluster, placed);
            pen += style.spacing;
            rest = &rest[cluster.len()..];
        }
        Ok(run)
    }

    /// Where `run` ends on the page.
    fn end(&self, run: &GlyphRun) -> f64 {
        run.glyphs().last().map_or(run.x().0, |last| {
            run.x().0 + last.x().0 + self.advance(run, last.glyph())
        })
    }

    /// The advance of `glyph` in `run`.
    fn advance(&self, run: &GlyphRun, glyph: GlyphId) -> f64 {
        let units = f64::from(self.face.advance_width(glyph).unwrap_or(0));
        units * run.size().0 * run.horizontal_scale() / f64::from(self.face.units_per_em())
    }

    /// `text` in small capitals: capitals at `size`, and the other letters drawn with the
    /// glyphs of their capitals at 80 % of it, each glyph keeping its own text.
    fn small_capitals(
        &self,
        text: &str,
        size: f64,
        mut x: f64,
        y: f64,
    ) -> Result<Vec<GlyphRun>, Box<dyn Error>> {
        let mut runs = Vec::new();
        let mut rest = text;
        while let Some(ch) = rest.chars().next() {
            let lower = !ch.is_uppercase();
            let length = rest
                .find(|c: char| c.is_uppercase() == lower)
                .unwrap_or(rest.len());
            let (part, after) = rest.split_at(length);
            let run = if lower {
                let small = Style {
                    capitals: true,
                    ..PLAIN
                };
                self.run(part, size * 0.8, x, y, &small)?
            } else {
                self.run(part, size, x, y, &PLAIN)?
            };
            x = self.end(&run);
            runs.push(run);
            rest = after;
        }
        Ok(runs)
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let (Some(font_path), Some(out)) = (args.next(), args.next()) else {
        return Err("usage: primitives FONT OUT.pdf".into());
    };
    let data = std::fs::read(&font_path)?;
    let face = FontFace::from_bytes(data.clone(), 0)?;
    let font = Font::from_bytes(data, 0)?;
    for note in font.embedding_notes() {
        eprintln!("warning: font {}: {note}", font.name());
    }
    let writer = Writer {
        face: &face,
        font: &font,
    };
    let mut doc = Document::new();
    let page = doc.add_page(PageSize::A4)?;

    // Text.
    let text = |text: &str, y: f64| writer.run(text, 12.0, 72.0, y, &PLAIN);
    page.draw_glyphs(&writer.run(
        "Glyph runs and vector primitives",
        18.0,
        72.0,
        780.0,
        &PLAIN,
    )?)?;
    page.draw_glyphs(&text("Ligatures: fine office flow", 740.0)?)?;
    let spaced = Style {
        spacing: 1.0,
        ..PLAIN
    };
    page.draw_glyphs(&writer.run("Letter spacing", 12.0, 72.0, 715.0, &spaced)?)?;
    let wide = Style {
        scale: 1.5,
        ..PLAIN
    };
    page.draw_glyphs(
        &writer
            .run("Scaled 150 %", 12.0, 72.0, 690.0, &wide)?
            .with_color(Color::rgb(0, 0, 192)),
    )?;
    page.draw_glyphs(&text("Slanted", 665.0)?.with_skew(0.25))?;
    page.draw_glyphs(&text("Stroked", 640.0)?.with_stroke(Pt(12.0 / 30.0)))?;
    page.draw_glyphs(&text("Café au lait…", 615.0)?)?;
    for run in writer.small_capitals("Small Caps", 12.0, 300.0, 640.0)? {
        page.draw_glyphs(&run)?;
    }
    let superscript = Style {
        raised: "2",
        rise: 4.0,
        ..PLAIN
    };
    page.draw_glyphs(&writer.run("E = mc2", 12.0, 300.0, 615.0, &superscript)?)?;

    // A highlight behind text.
    page.fill_path(
        &GraphicsPath::rect(Pt(72.0), Pt(560.0), Pt(200.0), Pt(20.0)),
        Color::rgb(255, 255, 0),
    )?;
    page.draw_glyphs(&writer.run("Highlighted text", 12.0, 76.0, 566.0, &PLAIN)?)?;

    // A dashed frame: dashes 6 pt long with gaps of 3 pt, from the lower-left corner.
    page.stroke_path(
        &GraphicsPath::rect(Pt(300.0), Pt(540.0), Pt(220.0), Pt(60.0)),
        &Stroke::new(Pt(2.0), Color::rgb(255, 0, 0)).with_dash(&[Pt(6.0), Pt(3.0)], Pt(0.0)),
    )?;

    // A dotted line: dashes of length 0 with round caps.
    let mut dotted = GraphicsPath::new(Pt(72.0), Pt(520.0));
    dotted.line_to(Pt(290.0), Pt(520.0));
    page.stroke_path(
        &dotted,
        &Stroke::new(Pt(2.0), Color::rgb(0, 128, 0))
            .with_dash(&[Pt(0.0), Pt(4.0)], Pt(0.0))
            .with_cap(LineCap::Round),
    )?;

    // A double line.
    for y in [500.0, 497.0] {
        let mut line = GraphicsPath::new(Pt(72.0), Pt(y));
        line.line_to(Pt(290.0), Pt(y));
        page.stroke_path(&line, &Stroke::new(Pt(0.75), Color::BLACK))?;
    }

    // A wavy line, of curves 8 pt long.
    let mut wave = GraphicsPath::new(Pt(72.0), Pt(470.0));
    let mut x = 72.0;
    while x < 288.0 {
        wave.curve_to(
            Pt(x + 2.0),
            Pt(474.0),
            Pt(x + 6.0),
            Pt(466.0),
            Pt(x + 8.0),
            Pt(470.0),
        );
        x += 8.0;
    }
    page.stroke_path(&wave, &Stroke::new(Pt(1.0), Color::rgb(0, 0, 255)))?;

    // A triangle, filled and outlined.
    let mut triangle = GraphicsPath::new(Pt(320.0), Pt(440.0));
    triangle
        .line_to(Pt(420.0), Pt(440.0))
        .line_to(Pt(370.0), Pt(520.0))
        .close();
    page.fill_path(&triangle, Color::rgb(192, 192, 192))?;
    page.stroke_path(&triangle, &Stroke::new(Pt(1.0), Color::BLACK))?;

    // A circle of radius 50 around (172, 350), clipped to its left half.
    page.push_clip(&GraphicsPath::rect(
        Pt(72.0),
        Pt(300.0),
        Pt(100.0),
        Pt(100.0),
    ))?;
    page.fill_path(&circle(172.0, 350.0, 50.0), Color::rgb(255, 0, 0))?;
    page.pop_clip()?;
    // Drawn after the clip is removed, outside it.
    page.fill_path(
        &GraphicsPath::rect(Pt(200.0), Pt(300.0), Pt(20.0), Pt(20.0)),
        Color::BLACK,
    )?;

    // Two nested clips: only where both squares overlap, from (350, 300) to (400, 350).
    page.push_clip(&GraphicsPath::rect(
        Pt(300.0),
        Pt(300.0),
        Pt(100.0),
        Pt(100.0),
    ))?;
    page.push_clip(&GraphicsPath::rect(
        Pt(350.0),
        Pt(250.0),
        Pt(100.0),
        Pt(100.0),
    ))?;
    page.fill_path(
        &GraphicsPath::rect(Pt(250.0), Pt(200.0), Pt(300.0), Pt(300.0)),
        Color::rgb(0, 0, 255),
    )?;
    page.pop_clip()?;
    page.pop_clip()?;

    doc.save(out)?;
    Ok(())
}

/// A circle as four cubic Bézier curves.
fn circle(cx: f64, cy: f64, r: f64) -> GraphicsPath {
    // The control points' distance that makes a quarter circle closest.
    let k = r * 0.552_284_75;
    let mut path = GraphicsPath::new(Pt(cx + r), Pt(cy));
    path.curve_to(
        Pt(cx + r),
        Pt(cy + k),
        Pt(cx + k),
        Pt(cy + r),
        Pt(cx),
        Pt(cy + r),
    )
    .curve_to(
        Pt(cx - k),
        Pt(cy + r),
        Pt(cx - r),
        Pt(cy + k),
        Pt(cx - r),
        Pt(cy),
    )
    .curve_to(
        Pt(cx - r),
        Pt(cy - k),
        Pt(cx - k),
        Pt(cy - r),
        Pt(cx),
        Pt(cy - r),
    )
    .curve_to(
        Pt(cx + k),
        Pt(cy - r),
        Pt(cx + r),
        Pt(cy - k),
        Pt(cx + r),
        Pt(cy),
    )
    .close();
    path
}
