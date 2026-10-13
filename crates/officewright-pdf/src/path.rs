//! Paths and the strokes that draw them, written as path construction and painting
//! operators (ISO 32000-1, 8.5).

use std::fmt::Write as _;

use officewright_core::{Color, Pt};

use crate::document::{MAX_POSITION, check_position, number};
use crate::{Error, Result};

/// The most lengths a dash pattern may have: the most elements an array may have in PDF 1.4
/// (PDF Reference 1.4, Appendix C).
const MAX_DASH_LENGTHS: usize = 8191;

/// A shape made of straight lines and cubic Bézier curves, to fill, stroke or clip with:
/// [`Page::fill_path`](crate::Page::fill_path), [`Page::stroke_path`](crate::Page::stroke_path)
/// and [`Page::push_clip`](crate::Page::push_clip).
///
/// Coordinates are page coordinates in points, as for [`Page::add_text`](crate::Page::add_text):
/// the origin is the bottom-left corner of the page and `y` grows upwards. A path is made of
/// subpaths; each starts at a point, and [`close`](Self::close) joins its end to that
/// point. Filling and clipping use the nonzero winding number rule (ISO 32000-1, 8.5.3.3.2).
///
/// ```
/// use officewright_pdf::{Color, Document, GraphicsPath, PageSize, Pt, Stroke};
///
/// // A triangle, filled red and outlined in black.
/// let mut triangle = GraphicsPath::new(Pt(100.0), Pt(100.0));
/// triangle.line_to(Pt(200.0), Pt(100.0)).line_to(Pt(150.0), Pt(180.0)).close();
///
/// let mut doc = Document::new();
/// let page = doc.add_page(PageSize::A4)?;
/// page.fill_path(&triangle, Color::rgb(255, 0, 0))?;
/// page.stroke_path(&triangle, &Stroke::new(Pt(1.0), Color::BLACK))?;
/// # Ok::<(), officewright_pdf::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct GraphicsPath {
    segments: Vec<Segment>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Segment {
    Move(Pt, Pt),
    Line(Pt, Pt),
    Curve([Pt; 6]),
    Close,
    /// Lower-left corner, width and height.
    Rect([Pt; 4]),
}

impl GraphicsPath {
    /// A path whose first subpath starts at (`x`, `y`).
    pub fn new(x: Pt, y: Pt) -> Self {
        Self {
            segments: vec![Segment::Move(x, y)],
        }
    }

    /// A path that is the rectangle with its lower-left corner at (`x`, `y`) and the given
    /// width and height, as one closed subpath.
    ///
    /// ```
    /// use officewright_pdf::{Color, Document, GraphicsPath, PageSize, Pt};
    ///
    /// let mut doc = Document::new();
    /// doc.add_page(PageSize::A4)?
    ///     .fill_path(&GraphicsPath::rect(Pt(72.0), Pt(700.0), Pt(144.0), Pt(20.0)), Color::rgb(255, 255, 0))?;
    /// # Ok::<(), officewright_pdf::Error>(())
    /// ```
    pub fn rect(x: Pt, y: Pt, width: Pt, height: Pt) -> Self {
        Self {
            segments: vec![Segment::Rect([x, y, width, height])],
        }
    }

    /// Starts a new subpath at (`x`, `y`).
    pub fn move_to(&mut self, x: Pt, y: Pt) -> &mut Self {
        self.segments.push(Segment::Move(x, y));
        self
    }

    /// Adds a straight line from the current point to (`x`, `y`).
    pub fn line_to(&mut self, x: Pt, y: Pt) -> &mut Self {
        self.segments.push(Segment::Line(x, y));
        self
    }

    /// Adds a cubic Bézier curve from the current point to (`x`, `y`), with control points
    /// (`x1`, `y1`) and (`x2`, `y2`).
    pub fn curve_to(&mut self, x1: Pt, y1: Pt, x2: Pt, y2: Pt, x: Pt, y: Pt) -> &mut Self {
        self.segments.push(Segment::Curve([x1, y1, x2, y2, x, y]));
        self
    }

    /// Closes the current subpath with a straight line back to its start, which becomes
    /// the current point.
    pub fn close(&mut self) -> &mut Self {
        self.segments.push(Segment::Close);
        self
    }

    /// Adds the rectangle with its lower-left corner at (`x`, `y`) and the given width and
    /// height as a closed subpath of its own; the current point becomes (`x`, `y`).
    pub fn add_rect(&mut self, x: Pt, y: Pt, width: Pt, height: Pt) -> &mut Self {
        self.segments.push(Segment::Rect([x, y, width, height]));
        self
    }

    /// Fails unless every point of the path, the corners of its rectangles included, is
    /// within -32767 to 32767 pt.
    pub(crate) fn check(&self) -> Result<()> {
        for segment in &self.segments {
            match *segment {
                Segment::Move(x, y) | Segment::Line(x, y) => check_position(x, y)?,
                Segment::Curve([x1, y1, x2, y2, x, y]) => {
                    check_position(x1, y1)?;
                    check_position(x2, y2)?;
                    check_position(x, y)?;
                }
                Segment::Close => {}
                Segment::Rect([x, y, width, height]) => {
                    check_position(x, y)?;
                    check_position(Pt(x.0 + width.0), Pt(y.0 + height.0))?;
                }
            }
        }
        Ok(())
    }

    /// Writes the path construction operators, each on a line of its own.
    pub(crate) fn write(&self, out: &mut String) {
        let points = |out: &mut String, points: &[Pt]| {
            for point in points {
                out.push_str(&number(point.0));
                out.push(' ');
            }
        };
        for segment in &self.segments {
            match segment {
                Segment::Move(x, y) => {
                    points(out, &[*x, *y]);
                    out.push_str("m\n");
                }
                Segment::Line(x, y) => {
                    points(out, &[*x, *y]);
                    out.push_str("l\n");
                }
                Segment::Curve(curve) => {
                    points(out, curve);
                    out.push_str("c\n");
                }
                Segment::Close => out.push_str("h\n"),
                Segment::Rect(rect) => {
                    points(out, rect);
                    out.push_str("re\n");
                }
            }
        }
    }
}

/// How a line or the outline of a path is stroked: its width, colour, dash pattern, and
/// the shape of its ends and corners.
///
/// A new stroke is solid, with butt caps and mitred joins, the PDF defaults.
///
/// ```
/// use officewright_pdf::{Color, Document, GraphicsPath, LineCap, PageSize, Pt, Stroke};
///
/// // A dotted line: dots 1 pt long every 3 pt.
/// let dotted = Stroke::new(Pt(1.0), Color::rgb(0, 0, 255))
///     .with_dash(&[Pt(1.0), Pt(2.0)], Pt(0.0))
///     .with_cap(LineCap::Butt);
/// let mut line = GraphicsPath::new(Pt(72.0), Pt(500.0));
/// line.line_to(Pt(300.0), Pt(500.0));
///
/// let mut doc = Document::new();
/// doc.add_page(PageSize::A4)?.stroke_path(&line, &dotted)?;
/// # Ok::<(), officewright_pdf::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    width: Pt,
    color: Color,
    dash: Vec<Pt>,
    dash_phase: Pt,
    cap: LineCap,
    join: LineJoin,
}

/// The shape of the ends of a stroked open subpath and of its dashes (ISO 32000-1,
/// 8.4.3.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LineCap {
    /// The stroke ends square at the end point.
    #[default]
    Butt,
    /// A semicircle of the line's width is drawn around the end point.
    Round,
    /// The stroke goes on half the line's width beyond the end point and ends square.
    Square,
}

/// The shape of the corners where two segments of a stroked path meet (ISO 32000-1,
/// 8.4.3.4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LineJoin {
    /// The outer edges are extended until they meet, as long as the corner is not
    /// sharper than about 11 degrees (the miter limit of 10); a sharper one is bevelled.
    #[default]
    Miter,
    /// A circle of the line's width is drawn around the corner.
    Round,
    /// The corner is cut off square.
    Bevel,
}

impl Stroke {
    /// A solid stroke `width` points wide in `color`. A width of 0 draws the thinnest line
    /// the device can show.
    pub fn new(width: Pt, color: Color) -> Self {
        Self {
            width,
            color,
            dash: Vec::new(),
            dash_phase: Pt(0.0),
            cap: LineCap::default(),
            join: LineJoin::default(),
        }
    }

    /// Dashes the stroke: `lengths` alternate between the lengths of dashes and of the gaps
    /// between them, in points, and repeat; `phase` is how far into the pattern the line
    /// starts. An empty pattern is a solid line. The caps are drawn at the ends of every
    /// dash, so round caps with dashes of length 0 draw dots.
    pub fn with_dash(mut self, lengths: &[Pt], phase: Pt) -> Self {
        self.dash = lengths.to_vec();
        self.dash_phase = phase;
        self
    }

    /// Sets the shape of the ends of open subpaths and dashes.
    pub fn with_cap(mut self, cap: LineCap) -> Self {
        self.cap = cap;
        self
    }

    /// Sets the shape of the corners.
    pub fn with_join(mut self, join: LineJoin) -> Self {
        self.join = join;
        self
    }

    /// The line width.
    pub fn width(&self) -> Pt {
        self.width
    }

    /// The colour.
    pub fn color(&self) -> Color {
        self.color
    }

    /// The dash pattern's lengths of dashes and gaps; empty for a solid line.
    pub fn dash(&self) -> &[Pt] {
        &self.dash
    }

    /// How far into the dash pattern the line starts.
    pub fn dash_phase(&self) -> Pt {
        self.dash_phase
    }

    /// The shape of the ends.
    pub fn cap(&self) -> LineCap {
        self.cap
    }

    /// The shape of the corners.
    pub fn join(&self) -> LineJoin {
        self.join
    }

    /// Fails unless the width, every dash length and the phase are within 0 to 32767 pt,
    /// the pattern has at most 8191 lengths, and not all of them are 0.
    pub(crate) fn check(&self) -> Result<()> {
        check_length("line width", self.width)?;
        if self.dash.len() > MAX_DASH_LENGTHS {
            return Err(Error::OutOfRange {
                what: "number of dash lengths",
                value: self.dash.len() as f64,
                min: 0.0,
                max: MAX_DASH_LENGTHS as f64,
            });
        }
        for &length in &self.dash {
            check_length("dash length", length)?;
        }
        check_length("dash phase", self.dash_phase)?;
        if !self.dash.is_empty() && self.dash.iter().all(|length| length.0 == 0.0) {
            return Err(Error::InvalidDash);
        }
        Ok(())
    }

    /// Writes the operators that set the stroke's line width, caps, joins, dash pattern
    /// and colour, leaving out the PDF defaults (ISO 32000-1, 8.4.4).
    pub(crate) fn write(&self, out: &mut String) {
        let _ = writeln!(out, "{} w", number(self.width.0));
        match self.cap {
            LineCap::Butt => {}
            LineCap::Round => out.push_str("1 J\n"),
            LineCap::Square => out.push_str("2 J\n"),
        }
        match self.join {
            LineJoin::Miter => {}
            LineJoin::Round => out.push_str("1 j\n"),
            LineJoin::Bevel => out.push_str("2 j\n"),
        }
        if !self.dash.is_empty() {
            let lengths: Vec<String> = self.dash.iter().map(|length| number(length.0)).collect();
            let _ = writeln!(
                out,
                "[{}] {} d",
                lengths.join(" "),
                number(self.dash_phase.0)
            );
        }
        let _ = writeln!(out, "{} RG", color(self.color));
    }
}

/// Fails unless `length` is within 0 to 32767 pt, the largest real number every PDF 1.4
/// reader has to handle.
pub(crate) fn check_length(what: &'static str, length: Pt) -> Result<()> {
    check_range(what, length.0, 0.0, MAX_POSITION)
}

/// Fails unless `value` is within `min` to `max`.
pub(crate) fn check_range(what: &'static str, value: f64, min: f64, max: f64) -> Result<()> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(Error::OutOfRange {
            what,
            value,
            min,
            max,
        })
    }
}

/// The operands of `rg` and `RG` for an sRGB colour, each component from 0 to 1. Four
/// decimals tell all 256 values of a component apart.
pub(crate) fn color(color: Color) -> String {
    let component = |value: u8| number(f64::from(value) / 255.0);
    format!(
        "{} {} {}",
        component(color.red()),
        component(color.green()),
        component(color.blue())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(path: &GraphicsPath) -> String {
        let mut out = String::new();
        path.write(&mut out);
        out
    }

    #[test]
    fn writes_every_kind_of_segment() {
        let mut path = GraphicsPath::new(Pt(1.0), Pt(2.0));
        path.line_to(Pt(3.5), Pt(-4.0))
            .curve_to(Pt(5.0), Pt(6.0), Pt(7.0), Pt(8.0), Pt(9.0), Pt(10.0))
            .close()
            .move_to(Pt(0.0), Pt(0.0))
            .add_rect(Pt(10.0), Pt(20.0), Pt(30.0), Pt(-40.0));
        assert_eq!(
            written(&path),
            "1 2 m\n3.5 -4 l\n5 6 7 8 9 10 c\nh\n0 0 m\n10 20 30 -40 re\n"
        );
        assert_eq!(
            written(&GraphicsPath::rect(Pt(1.0), Pt(2.0), Pt(3.0), Pt(4.0))),
            "1 2 3 4 re\n"
        );
    }

    #[test]
    fn checks_every_point_and_both_corners_of_rectangles() {
        let ok = Pt(32767.0);
        let far = Pt(32767.5);
        assert!(GraphicsPath::new(ok, Pt(-32767.0)).check().is_ok());
        assert!(GraphicsPath::rect(Pt(0.0), Pt(0.0), ok, ok).check().is_ok());

        let mut cases = vec![
            GraphicsPath::new(far, Pt(0.0)),
            GraphicsPath::new(Pt(0.0), Pt(f64::NAN)),
            GraphicsPath::rect(Pt(1.0), Pt(0.0), ok, Pt(0.0)),
            GraphicsPath::rect(Pt(0.0), Pt(0.0), Pt(0.0), Pt(f64::INFINITY)),
        ];
        for at in 0..3 {
            let mut points = [Pt(0.0); 6];
            points[2 * at] = far;
            let mut path = GraphicsPath::new(Pt(0.0), Pt(0.0));
            path.curve_to(
                points[0], points[1], points[2], points[3], points[4], points[5],
            );
            cases.push(path);
        }
        let mut line = GraphicsPath::new(Pt(0.0), Pt(0.0));
        line.line_to(Pt(0.0), Pt(-32768.0));
        cases.push(line);
        let mut moved = GraphicsPath::new(Pt(0.0), Pt(0.0));
        moved.close().move_to(far, Pt(0.0));
        cases.push(moved);
        for path in cases {
            let err = path.check().unwrap_err();
            assert!(matches!(err, Error::InvalidPosition { .. }), "{path:?}");
        }
    }

    #[test]
    fn writes_only_what_differs_from_the_defaults() {
        let mut out = String::new();
        Stroke::new(Pt(0.5), Color::BLACK).write(&mut out);
        assert_eq!(out, "0.5 w\n0 0 0 RG\n");

        let mut out = String::new();
        Stroke::new(Pt(2.0), Color::rgb(255, 128, 1))
            .with_dash(&[Pt(3.0), Pt(1.5)], Pt(0.25))
            .with_cap(LineCap::Round)
            .with_join(LineJoin::Bevel)
            .write(&mut out);
        assert_eq!(out, "2 w\n1 J\n2 j\n[3 1.5] 0.25 d\n1 0.502 0.0039 RG\n");

        let mut out = String::new();
        Stroke::new(Pt(1.0), Color::WHITE)
            .with_cap(LineCap::Square)
            .with_join(LineJoin::Round)
            .write(&mut out);
        assert_eq!(out, "1 w\n2 J\n1 j\n1 1 1 RG\n");
    }

    #[test]
    fn colours_keep_every_component_value_apart() {
        let written: Vec<f64> = (0..=255u8)
            .map(|value| number(f64::from(value) / 255.0).parse().unwrap())
            .collect();
        for (value, component) in written.iter().enumerate() {
            assert_eq!((component * 255.0).round() as usize, value);
        }
        assert_eq!(color(Color::rgb(0, 51, 255)), "0 0.2 1");
    }

    #[test]
    fn checks_strokes() {
        let stroke = |width: f64| Stroke::new(Pt(width), Color::BLACK);
        assert!(stroke(0.0).check().is_ok());
        assert!(stroke(32767.0).check().is_ok());
        assert!(
            stroke(1.0)
                .with_dash(&[Pt(0.0), Pt(2.0)], Pt(32767.0))
                .check()
                .is_ok()
        );
        assert!(stroke(1.0).with_dash(&[], Pt(0.0)).check().is_ok());
        let longest = vec![Pt(1.0); MAX_DASH_LENGTHS];
        assert!(stroke(1.0).with_dash(&longest, Pt(0.0)).check().is_ok());

        let out_of_range = [
            (stroke(-0.1), "line width"),
            (stroke(f64::NAN), "line width"),
            (stroke(32767.5), "line width"),
            (
                stroke(1.0).with_dash(&[Pt(1.0), Pt(-1.0)], Pt(0.0)),
                "dash length",
            ),
            (
                stroke(1.0).with_dash(&[Pt(f64::INFINITY)], Pt(0.0)),
                "dash length",
            ),
            (stroke(1.0).with_dash(&[Pt(1.0)], Pt(-1.0)), "dash phase"),
            (
                stroke(1.0).with_dash(&[Pt(1.0)], Pt(f64::NAN)),
                "dash phase",
            ),
            (
                stroke(1.0).with_dash(&vec![Pt(1.0); MAX_DASH_LENGTHS + 1], Pt(0.0)),
                "number of dash lengths",
            ),
        ];
        for (stroke, expected) in out_of_range {
            match stroke.check() {
                Err(Error::OutOfRange { what, .. }) => assert_eq!(what, expected),
                other => panic!("{stroke:?}: {other:?}"),
            }
        }
        assert!(matches!(
            stroke(1.0).with_dash(&[Pt(0.0), Pt(0.0)], Pt(0.0)).check(),
            Err(Error::InvalidDash)
        ));
    }

    #[test]
    fn new_strokes_have_the_pdf_defaults() {
        let stroke = Stroke::new(Pt(1.5), Color::WHITE);
        assert_eq!(stroke.width(), Pt(1.5));
        assert_eq!(stroke.color(), Color::WHITE);
        assert!(stroke.dash().is_empty());
        assert_eq!(stroke.dash_phase(), Pt(0.0));
        assert_eq!(stroke.cap(), LineCap::Butt);
        assert_eq!(stroke.join(), LineJoin::Miter);
    }

    #[test]
    fn range_errors_name_the_number() {
        let err = check_range("skew", f64::NAN, -1.0, 1.0).unwrap_err();
        assert_eq!(err.to_string(), "skew NaN is outside the -1 to 1 range");
        let err = check_length("line width", Pt(-2.5)).unwrap_err();
        assert_eq!(
            err.to_string(),
            "line width -2.5 is outside the 0 to 32767 range"
        );
    }
}
