//! Blocks and paragraphs, with the properties that shape them.

use officewright_core::Pt;

use super::RunStyleId;
use super::inline::Inline;
use super::macros::{ir_struct, no_rules};
use super::rules;
use super::style::ColorValue;
use super::walk::{Check, Checker, Clamp, Clamper};
use crate::Result;

/// A block of the text flow.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Block {
    /// A paragraph.
    Paragraph(Paragraph),
}

impl From<Paragraph> for Block {
    fn from(paragraph: Paragraph) -> Self {
        Self::Paragraph(paragraph)
    }
}

impl Check for Block {
    fn check(&self, checker: &mut Checker<'_>) -> Result<()> {
        match self {
            Self::Paragraph(paragraph) => paragraph.check(checker),
        }
    }
}

impl Clamp for Block {
    fn clamp(&mut self, clamper: &mut Clamper) {
        match self {
            Self::Paragraph(paragraph) => paragraph.clamp(clamper),
        }
    }
}

ir_struct! {
    /// A paragraph: its properties, and the text and objects in it.
    #[derive(Clone, Debug, PartialEq)]
    pub struct Paragraph {
        /// How the paragraph is laid out.
        ref props: ParagraphProps => with_props;
        /// The text and objects, in reading order.
        vec inlines: Inline => with_inlines;
        /// The formatting of the paragraph mark. It gives an empty paragraph its height,
        /// and the last line of a paragraph includes it in its height.
        copy mark: RunStyleId => with_mark;
    }
}

impl Paragraph {
    /// An empty paragraph with default properties, whose mark has this style.
    pub fn new(mark: RunStyleId) -> Self {
        Self {
            props: ParagraphProps::default(),
            inlines: Vec::new(),
            mark,
        }
    }
}

ir_struct! {
    /// How a paragraph is laid out, with the styles already applied. Each default is
    /// what WordprocessingML means when nothing in the style hierarchy sets the property.
    #[derive(Clone, Debug, PartialEq)]
    pub struct ParagraphProps {
        /// How the lines are aligned. Start when nothing sets it (MS-OI29500 2.1.45 a).
        copy alignment: Alignment = Alignment::Start => with_alignment;
        /// The indents.
        copy indent: Indent = Indent::default() => with_indent;
        /// The space above and below the paragraph and between its lines.
        copy spacing: Spacing = Spacing::default() => with_spacing;
        /// Whether the space above and below is left out next to a paragraph of the same
        /// paragraph style (see [`style_key`](Self::style_key)).
        copy contextual_spacing: bool = false => with_contextual_spacing;
        /// Whether the paragraph stays on the page of the start of the next block.
        copy keep_next: bool = false => with_keep_next;
        /// Whether all lines of the paragraph stay on one page.
        copy keep_lines: bool = false => with_keep_lines;
        /// Whether a page break leaves at least two lines of the paragraph on each page.
        copy widow_control: bool = false => with_widow_control;
        /// Whether the paragraph starts a new page.
        copy page_break_before: bool = false => with_page_break_before;
        /// The custom tab stops, with those of the styles merged in and cleared ones
        /// removed.
        vec tabs: TabStop = Vec::new() => with_tabs;
        /// The borders.
        ref borders: ParagraphBorders = ParagraphBorders::default() => with_borders;
        /// The shading behind the paragraph.
        copy shading: Option<Shading> = None => with_shading;
        /// Whether lines break only between words. When false, a word that does not fit
        /// breaks at any character (`w:wordWrap`).
        copy word_wrap: bool = true => with_word_wrap;
        /// Whether automatic hyphenation leaves the paragraph alone.
        copy suppress_auto_hyphens: bool = false => with_suppress_auto_hyphens;
        /// The outline level, 0 for the top level, or `None` for body text.
        copy outline_level: Option<u8> = None => with_outline_level;
        /// The paragraph style, as a key that is only compared for equality, for
        /// [`contextual_spacing`](Self::contextual_spacing).
        copy style_key: StyleKey = StyleKey::default() => with_style_key;
    }
}

/// How the lines of a paragraph are aligned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Alignment {
    /// At the start edge: the left in left-to-right text.
    #[default]
    Start,
    /// Centred.
    Center,
    /// At the end edge.
    End,
    /// Justified: every line but the last fills the width, by widening the spaces.
    Both,
    /// Every line, the last one too, fills the width, by spacing out the characters.
    Distribute,
}

ir_struct! {
    /// The indents of a paragraph, from the edges of its column.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Indent {
        /// From the start edge. Negative moves the text into the margin.
        copy start: Pt = Pt(0.0) => with_start;
        /// From the end edge. Negative moves the text into the margin.
        copy end: Pt = Pt(0.0) => with_end;
        /// The first line's indent, from [`start`](Self::start): positive indents it
        /// further (`w:firstLine`), negative makes it hang out (`w:hanging`).
        copy first_line: Pt = Pt(0.0) => with_first_line;
    }
}

ir_struct! {
    /// The space above and below a paragraph, and between its lines.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Spacing {
        /// Above the paragraph.
        copy before: Pt = Pt(0.0) => with_before, check rules::non_negative;
        /// Below the paragraph.
        copy after: Pt = Pt(0.0) => with_after, check rules::non_negative;
        /// Whether the space above is chosen automatically, as in HTML, instead of
        /// [`before`](Self::before).
        copy before_auto: bool = false => with_before_auto;
        /// Whether the space below is chosen automatically instead of
        /// [`after`](Self::after).
        copy after_auto: bool = false => with_after_auto;
        /// The height of the lines.
        copy line: LineSpacing = LineSpacing::Multiple(1.0) => with_line;
    }
}

/// The height of the lines of a paragraph.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum LineSpacing {
    /// A multiple of the height of single lines, which the fonts give: 1.0 is single,
    /// 2.0 double (`w:lineRule="auto"`, `w:line` divided by 240).
    Multiple(f64),
    /// Exactly this height, whatever the fonts.
    Exact(Pt),
    /// The height of single lines, but at least this.
    AtLeast(Pt),
}

impl Default for LineSpacing {
    fn default() -> Self {
        Self::Multiple(1.0)
    }
}

impl Check for LineSpacing {
    fn check(&self, checker: &mut Checker<'_>) -> Result<()> {
        match self {
            Self::Multiple(factor) if !factor.is_finite() => Err(checker.error(format!(
                "line spacing factor {factor} is not a finite number"
            ))),
            Self::Multiple(_) => Ok(()),
            Self::Exact(height) | Self::AtLeast(height) => height.check(checker),
        }
    }
}

impl Clamp for LineSpacing {
    fn clamp(&mut self, _: &mut Clamper) {}
}

ir_struct! {
    /// A custom tab stop.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct TabStop {
        /// Where the stop is, from the start edge of the column.
        copy position: Pt = Pt(0.0) => with_position, clamp rules::tab_position;
        /// How the text after the tab aligns at the stop.
        copy kind: TabKind = TabKind::Start => with_kind;
        /// What fills the space before the stop.
        copy leader: TabLeader = TabLeader::None => with_leader;
    }
}

/// How the text after a tab aligns at its stop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TabKind {
    /// The text starts at the stop.
    #[default]
    Start,
    /// The text is centred on the stop.
    Center,
    /// The text ends at the stop.
    End,
    /// The decimal separator of the number after the tab is at the stop.
    Decimal,
    /// A vertical line is drawn at the stop; the text does not align to it.
    Bar,
}

/// What fills the space before a tab stop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TabLeader {
    /// Nothing.
    #[default]
    None,
    /// Dots.
    Dot,
    /// Hyphens.
    Hyphen,
    /// A line, as of underscores.
    Underscore,
    /// A thick line.
    Heavy,
    /// Dots at the middle of the line.
    MiddleDot,
}

ir_struct! {
    /// The borders of a paragraph.
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct ParagraphBorders {
        /// Above the paragraph.
        copy top: Option<Border> => with_top;
        /// Below the paragraph.
        copy bottom: Option<Border> => with_bottom;
        /// At the start edge.
        copy start: Option<Border> => with_start;
        /// At the end edge.
        copy end: Option<Border> => with_end;
        /// Between this paragraph and the next when both have the same borders.
        copy between: Option<Border> => with_between;
        /// A vertical line beside the paragraph, on the left of odd pages and the right of
        /// even ones.
        copy bar: Option<Border> => with_bar;
    }
}

ir_struct! {
    /// A border line.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Border {
        /// The style of the line.
        copy style: BorderStyle = BorderStyle::Single => with_style;
        /// The width of the line.
        copy width: Pt = Pt(0.5) => with_width, check rules::non_negative;
        /// The distance from the text.
        copy space: Pt = Pt(0.0) => with_space, check rules::non_negative;
        /// The colour.
        copy color: ColorValue = ColorValue::Auto => with_color;
        /// Whether the line has a shadow.
        copy shadow: bool = false => with_shadow;
    }
}

/// The style of a border line (ST_Border, without the art borders of pages).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BorderStyle {
    /// `single`: a single line.
    #[default]
    Single,
    /// `thick`: a single thick line.
    Thick,
    /// `double`: two lines.
    Double,
    /// `dotted`: dots.
    Dotted,
    /// `dashed`: dashes.
    Dashed,
    /// `dotDash`: dots and dashes in turn.
    DotDash,
    /// `dotDotDash`: two dots, then a dash.
    DotDotDash,
    /// `triple`: three lines.
    Triple,
    /// `thinThickSmallGap`: a thin line, then a thick one, with a small gap.
    ThinThickSmallGap,
    /// `thickThinSmallGap`: a thick line, then a thin one, with a small gap.
    ThickThinSmallGap,
    /// `thinThickThinSmallGap`: thin, thick and thin lines with small gaps.
    ThinThickThinSmallGap,
    /// `thinThickMediumGap`: a thin line, then a thick one, with a medium gap.
    ThinThickMediumGap,
    /// `thickThinMediumGap`: a thick line, then a thin one, with a medium gap.
    ThickThinMediumGap,
    /// `thinThickThinMediumGap`: thin, thick and thin lines with medium gaps.
    ThinThickThinMediumGap,
    /// `thinThickLargeGap`: a thin line, then a thick one, with a large gap.
    ThinThickLargeGap,
    /// `thickThinLargeGap`: a thick line, then a thin one, with a large gap.
    ThickThinLargeGap,
    /// `thinThickThinLargeGap`: thin, thick and thin lines with large gaps.
    ThinThickThinLargeGap,
    /// `wave`: a wavy line.
    Wave,
    /// `doubleWave`: two wavy lines.
    DoubleWave,
    /// `dashSmallGap`: dashes with small gaps.
    DashSmallGap,
    /// `dashDotStroked`: dashes and dots, stroked.
    DashDotStroked,
    /// `threeDEmboss`: raised in three dimensions.
    ThreeDEmboss,
    /// `threeDEngrave`: sunk in three dimensions.
    ThreeDEngrave,
    /// `outset`: lit as if raised.
    Outset,
    /// `inset`: lit as if sunk.
    Inset,
}

ir_struct! {
    /// A shading: a pattern in a colour over a fill.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Shading {
        /// The pattern.
        copy pattern: ShadingPattern = ShadingPattern::Clear => with_pattern;
        /// The colour of the pattern. Automatic is black.
        copy color: ColorValue = ColorValue::Auto => with_color;
        /// The fill under the pattern. Automatic is no fill.
        copy fill: ColorValue = ColorValue::Auto => with_fill;
    }
}

/// The pattern of a shading (ST_Shd): a share of the pattern colour mixed into the fill,
/// or stripes and crosses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ShadingPattern {
    /// `clear`: no pattern: the fill alone.
    #[default]
    Clear,
    /// `solid`: the pattern colour alone.
    Solid,
    /// `horzStripe`: horizontal stripes.
    HorizontalStripe,
    /// `vertStripe`: vertical stripes.
    VerticalStripe,
    /// `reverseDiagStripe`: stripes from top left to bottom right.
    ReverseDiagonalStripe,
    /// `diagStripe`: stripes from bottom left to top right.
    DiagonalStripe,
    /// `horzCross`: horizontal and vertical lines.
    HorizontalCross,
    /// `diagCross`: crossed diagonal lines.
    DiagonalCross,
    /// `thinHorzStripe`: thin horizontal stripes.
    ThinHorizontalStripe,
    /// `thinVertStripe`: thin vertical stripes.
    ThinVerticalStripe,
    /// `thinReverseDiagStripe`: thin stripes from top left to bottom right.
    ThinReverseDiagonalStripe,
    /// `thinDiagStripe`: thin stripes from bottom left to top right.
    ThinDiagonalStripe,
    /// `thinHorzCross`: thin horizontal and vertical lines.
    ThinHorizontalCross,
    /// `thinDiagCross`: thin crossed diagonal lines.
    ThinDiagonalCross,
    /// `pct5`: 5 % of the pattern colour over the fill.
    Percent5,
    /// `pct10`: 10 % of the pattern colour over the fill.
    Percent10,
    /// `pct12`: 12 % of the pattern colour over the fill.
    Percent12,
    /// `pct15`: 15 % of the pattern colour over the fill.
    Percent15,
    /// `pct20`: 20 % of the pattern colour over the fill.
    Percent20,
    /// `pct25`: 25 % of the pattern colour over the fill.
    Percent25,
    /// `pct30`: 30 % of the pattern colour over the fill.
    Percent30,
    /// `pct35`: 35 % of the pattern colour over the fill.
    Percent35,
    /// `pct37`: 37 % of the pattern colour over the fill.
    Percent37,
    /// `pct40`: 40 % of the pattern colour over the fill.
    Percent40,
    /// `pct45`: 45 % of the pattern colour over the fill.
    Percent45,
    /// `pct50`: 50 % of the pattern colour over the fill.
    Percent50,
    /// `pct55`: 55 % of the pattern colour over the fill.
    Percent55,
    /// `pct60`: 60 % of the pattern colour over the fill.
    Percent60,
    /// `pct62`: 62 % of the pattern colour over the fill.
    Percent62,
    /// `pct65`: 65 % of the pattern colour over the fill.
    Percent65,
    /// `pct70`: 70 % of the pattern colour over the fill.
    Percent70,
    /// `pct75`: 75 % of the pattern colour over the fill.
    Percent75,
    /// `pct80`: 80 % of the pattern colour over the fill.
    Percent80,
    /// `pct85`: 85 % of the pattern colour over the fill.
    Percent85,
    /// `pct87`: 87 % of the pattern colour over the fill.
    Percent87,
    /// `pct90`: 90 % of the pattern colour over the fill.
    Percent90,
    /// `pct95`: 95 % of the pattern colour over the fill.
    Percent95,
}

/// A paragraph style, as a key that is only compared for equality. The producer gives
/// each paragraph style a key of its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct StyleKey(u32);

impl StyleKey {
    /// The key with this number.
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// The number of the key.
    pub const fn value(self) -> u32 {
        self.0
    }
}

no_rules!(
    Alignment,
    TabKind,
    TabLeader,
    BorderStyle,
    ShadingPattern,
    StyleKey
);
