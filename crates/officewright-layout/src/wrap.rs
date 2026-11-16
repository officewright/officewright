//! Text wrapped around floating objects (#511): the band of the page each floating object
//! of the body keeps text out of, and the room that leaves a line.
//!
//! Word's renders of `E-wrap-square.docx`, `E-wrap-top-bottom.docx`, `E-wrap-textbox.docx`
//! and `E-wrap-vml.docx` show the rules here. A line meets an object when its box, from its
//! top to its bottom, overlaps the object's band: from the top of the object's frame less
//! its distance above (`distT`) to the bottom of the frame and its distance below (`distB`).
//! The ninth line of case 01 of `E-wrap-square`, whose top is 0.56 pt above the bottom of
//! the picture, goes beside it, the tenth starts at the margin; in case 09, 36 pt below the
//! paragraph's top, the third line, whose bottom reaches 4.3 pt into the band, goes beside
//! it. A line beside an object wrapped square starts its distance at the right (`distR`)
//! past the frame, or ends its distance at the left (`distL`) before it: at 189 pt for a
//! picture of 108 pt at the margin of 72 pt and 9 pt (case 01), 180 with no distances
//! (case 07), 198 with 18 pt (case 08); case 02, at the right margin, ends the lines 9 pt
//! before the picture. Text goes on the side the object's [`WrapSide`] allows: the left
//! only (case 05), the right only (case 06), the side with more room (case 04). A line that
//! meets an object wrapped top and bottom goes below the band, its top at the band's bottom
//! (`E-wrap-top-bottom` cases 01, 02 with 12 pt below, 04 at 40 pt below the paragraph's
//! top, where the first two lines stay above it, and 06 from the page's top), even where
//! the object leaves room beside it (case 03, a picture of 72 pt centred).
//!
//! Hypotheses until Word renders rule documents for them: text goes on one side of an
//! object only (Word fills both, case 03 of `E-wrap-square`, which is reported), the larger
//! one, the left where both are as large; a side narrower than [`LEAST_ROOM`] takes no
//! text, and a line with no wider room goes below the band that ends first, as below an
//! object wrapped top and bottom; the paragraph's indents still hold beside an object, the
//! object's edge narrowing a line only where it is past the indent; tight and through
//! wrapping keep text from the frame, as square wrapping does (the polygon of a whole
//! picture gives the same lines, `E-wrap-tight` case 01 and `E-wrap-textbox` case 03, but
//! other polygons do not, which is reported); and the effects an object draws beyond its
//! frame (`wp:effectExtent`) do not widen its band.

use officewright_core::Pt;

use crate::ir::{AnchoredObject, Wrap, WrapSide};

/// Lengths closer than this are equal: a line that reaches a band only by rounding does
/// not meet it.
const EPSILON: f64 = 1e-6;

/// What the lines of a paragraph go around: the floating objects of its page.
pub(crate) struct Around<'a> {
    /// The first lines as laid out before, which stay as they are: the room above each,
    /// and the part of the column it had (`Line::above`, `Line::room`).
    pub(crate) kept: Vec<(Pt, Option<(Pt, Pt)>)>,
    /// What the floating objects keep text out of.
    pub(crate) exclusions: &'a [Exclusion],
    /// Where the column's start edge is across the page, and the top of the first line
    /// after those kept down it.
    pub(crate) origin: (f64, f64),
    /// The height of that line as laid out before, which fitting it starts from.
    pub(crate) estimate: f64,
}

/// The narrowest room beside a floating object that takes text. A hypothesis, as no Word
/// render has a narrower side than 111 pt (`E-wrap-square` case 04).
pub(crate) const LEAST_ROOM: f64 = 18.0;

/// The most floating objects of a page that text goes around; it runs over or under the
/// others, which is reported. [`room`] takes time in the cube of the objects a line meets
/// at worst, so that this bounds it; a page of a document seldom has more than a few.
pub(crate) const MAX_EXCLUSIONS: usize = 32;

/// What a floating object keeps text out of, in points from the page's top left corner: the
/// band down the page from the top of its frame less its distance above to the bottom of its
/// frame and its distance below, and across it the frame with its distances at the left and
/// right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Exclusion {
    pub(crate) top: f64,
    pub(crate) bottom: f64,
    pub(crate) left: f64,
    pub(crate) right: f64,
    /// The sides text may go on; `None` for neither, as text wrapped top and bottom.
    pub(crate) side: Option<WrapSide>,
}

impl Exclusion {
    /// What `object`, the top left corner of its frame at `at`, keeps text out of: nothing
    /// when text does not wrap around it.
    pub(crate) fn of(object: &AnchoredObject, (x, y): (f64, f64)) -> Option<Self> {
        let side = match object.wrap() {
            Wrap::None => return None,
            Wrap::TopAndBottom => None,
            Wrap::Square | Wrap::Tight | Wrap::Through => Some(object.wrap_side()),
        };
        let (size, distances) = (object.size(), object.distances());
        Some(Self {
            top: y - distances.top().0,
            bottom: y + size.height().0 + distances.bottom().0,
            left: x - distances.left().0,
            right: x + size.width().0 + distances.right().0,
            side,
        })
    }

    /// Whether a line from `top` to `bottom` down the page meets the band.
    pub(crate) fn meets(&self, top: f64, bottom: f64) -> bool {
        top < self.bottom - EPSILON && bottom > self.top + EPSILON
    }

    /// The parts of `span`, from the start of a line to its end across the page, where text
    /// may go beside the object, given the line's `edges`, by which the larger side is
    /// measured.
    fn beside(&self, span: (f64, f64), (start, end): (f64, f64), out: &mut Vec<(f64, f64)>) {
        let (left, right) = match self.side {
            None => (false, false),
            Some(WrapSide::Left) => (true, false),
            Some(WrapSide::Right) => (false, true),
            Some(WrapSide::Largest) => {
                let more_left = self.left - start >= end - self.right;
                (more_left, !more_left)
            }
            Some(WrapSide::BothSides) => (true, true),
        };
        let (from, to) = span;
        if left && to.min(self.left) > from {
            out.push((from, to.min(self.left)));
        }
        if right && to > from.max(self.right) {
            out.push((from.max(self.right), to));
        }
    }
}

/// Where a line goes around floating objects: how far below the place it would start it
/// goes, the part of the page across it takes where that is narrower than its edges, and
/// whether text had room on more than one side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Room {
    pub(crate) above: f64,
    pub(crate) span: Option<(f64, f64)>,
    pub(crate) sides: bool,
}

/// The room of a line `height` high whose top would be at `y` down the page, between
/// `edges`, its start and end across the page, beside `exclusions`. Where the objects it
/// meets leave it no side at least [`LEAST_ROOM`] wide, it goes below the band that ends
/// first among them, and is measured there again.
pub(crate) fn room(exclusions: &[Exclusion], y: f64, height: f64, edges: (f64, f64)) -> Room {
    let mut top = y;
    let mut spans = Vec::new();
    // Each round goes below the bottom of a band the line met, so after as many rounds as
    // there are bands it meets none.
    for _ in 0..=exclusions.len() {
        let met: Vec<&Exclusion> = exclusions
            .iter()
            .filter(|exclusion| exclusion.meets(top, top + height))
            .collect();
        if met.is_empty() {
            break;
        }
        spans.clear();
        spans.push(edges);
        for exclusion in &met {
            let mut next = Vec::with_capacity(spans.len() + 1);
            for &span in &spans {
                exclusion.beside(span, edges, &mut next);
            }
            spans = next;
        }
        spans.retain(|&(from, to)| to - from >= LEAST_ROOM);
        // The widest, the first of those as wide.
        let mut widest: Option<(f64, f64)> = None;
        for &(from, to) in &spans {
            if widest.is_none_or(|(a, b)| to - from > b - a) {
                widest = Some((from, to));
            }
        }
        if let Some(span) = widest {
            return Room {
                above: top - y,
                span: (span != edges).then_some(span),
                sides: spans.len() > 1,
            };
        }
        let below = met
            .iter()
            .map(|exclusion| exclusion.bottom)
            .fold(f64::INFINITY, f64::min);
        // A band that ends no lower (by a number that is not one) moves the line no
        // further.
        if below.partial_cmp(&top) != Some(std::cmp::Ordering::Greater) {
            break;
        }
        top = below;
    }
    Room {
        above: top - y,
        span: None,
        sides: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(side: WrapSide, (left, right): (f64, f64), (top, bottom): (f64, f64)) -> Exclusion {
        Exclusion {
            top,
            bottom,
            left,
            right,
            side: Some(side),
        }
    }

    #[test]
    fn a_line_meets_a_band_it_overlaps_by_more_than_rounding() {
        let band = square(WrapSide::BothSides, (0.0, 10.0), (100.0, 200.0));
        assert!(band.meets(199.0, 210.0));
        assert!(band.meets(90.0, 100.5));
        assert!(!band.meets(200.0, 210.0));
        assert!(!band.meets(90.0, 100.0));
        assert!(!band.meets(200.0 - 1e-9, 210.0));
    }

    #[test]
    fn a_line_goes_on_the_side_its_object_allows_with_room_for_it() {
        // A column from 72 to 523, an object from 263 to 389 with its distances.
        let edges = (72.0, 523.0);
        let at = |side| {
            room(
                &[square(side, (263.0, 389.0), (0.0, 100.0))],
                10.0,
                12.0,
                edges,
            )
        };
        let left = Room {
            above: 0.0,
            span: Some((72.0, 263.0)),
            sides: false,
        };
        let right = Room {
            span: Some((389.0, 523.0)),
            ..left
        };
        assert_eq!(at(WrapSide::Left), left);
        assert_eq!(at(WrapSide::Right), right);
        assert_eq!(at(WrapSide::Largest), left);
        assert_eq!(
            at(WrapSide::BothSides),
            Room {
                sides: true,
                ..left
            }
        );
        // Below the band, or above it, the line keeps its edges.
        let free = Room {
            above: 0.0,
            span: None,
            sides: false,
        };
        let band = [square(WrapSide::Left, (263.0, 389.0), (0.0, 100.0))];
        assert_eq!(room(&band, 100.0, 12.0, edges), free);
        assert_eq!(room(&band, -12.0, 12.0, edges), free);
        // An object outside the line's edges narrows nothing.
        let margin = [square(WrapSide::BothSides, (0.0, 60.0), (0.0, 100.0))];
        assert_eq!(room(&margin, 10.0, 12.0, edges), free);
    }

    #[test]
    fn a_line_with_no_room_beside_an_object_goes_below_it() {
        let edges = (72.0, 523.0);
        // Top and bottom: below the band, even with room beside it.
        let top_and_bottom = Exclusion {
            side: None,
            ..square(WrapSide::BothSides, (250.0, 340.0), (50.0, 122.0))
        };
        let below = room(&[top_and_bottom], 40.0, 13.0, edges);
        assert_eq!(
            below,
            Room {
                above: 82.0,
                span: None,
                sides: false
            }
        );
        // Square, with less than the least room on each side.
        let wide = square(
            WrapSide::BothSides,
            (72.0 + LEAST_ROOM - 1.0, 523.0 - LEAST_ROOM + 1.0),
            (0.0, 30.0),
        );
        assert_eq!(room(&[wide], 0.0, 13.0, edges).above, 30.0);
        // Below one band, the line meets another, beside which it has room.
        let lower = square(WrapSide::BothSides, (72.0, 200.0), (25.0, 60.0));
        assert_eq!(
            room(&[wide, lower], 0.0, 13.0, edges),
            Room {
                above: 30.0,
                span: Some((200.0, 523.0)),
                sides: false
            }
        );
    }
}
