//! Settings that apply to the whole document.

use officewright_core::Pt;

use super::macros::{ir_struct, no_rules};
use super::rules;

ir_struct! {
    /// Settings that apply to the whole document.
    #[derive(Clone, Debug, PartialEq)]
    pub struct DocumentSettings {
        /// The Word version whose layout rules apply. Word 2007 when the document does not
        /// say ([MS-DOCX] 2.3.5).
        copy compat_mode: CompatMode = CompatMode::Word2007 => with_compat_mode;
        /// The legacy compatibility options the document sets.
        vec compat_flags: CompatFlag = Vec::new() => with_compat_flags;
        /// The distance between the automatic tab stops that follow the last custom stop of
        /// a paragraph. Word's default is 36 pt.
        copy default_tab_stop: Pt = Pt(36.0) => with_default_tab_stop,
            check rules::non_negative, clamp rules::default_tab_stop;
        /// The character between the whole and the fractional part of a number, which
        /// decimal tab stops align on.
        copy decimal_separator: char = '.' => with_decimal_separator;
        /// Whether even pages have headers and footers of their own.
        copy even_and_odd_headers: bool = false => with_even_and_odd_headers;
        /// Whether the left and right margins of facing pages mirror each other. Not laid
        /// out yet.
        copy mirror_margins: bool = false => with_mirror_margins;
        /// Whether the gutter is at the top of the page instead of the left. Not laid out
        /// yet.
        copy gutter_at_top: bool = false => with_gutter_at_top;
        /// Automatic hyphenation.
        copy hyphenation: Hyphenation = Hyphenation::default() => with_hyphenation;
    }
}

impl DocumentSettings {
    /// Whether the document sets a compatibility option.
    pub fn has_compat_flag(&self, flag: &CompatFlag) -> bool {
        self.compat_flags.contains(flag)
    }
}

/// The Word version whose layout rules a document follows: Word lays out a document
/// saved in an older version's mode as that version did (the `compatibilityMode`
/// setting, [MS-DOCX] 2.3.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum CompatMode {
    /// Word 2003, mode 11.
    Word2003,
    /// Word 2007, mode 12.
    Word2007,
    /// Word 2010, mode 14.
    Word2010,
    /// Word 2013 and later, mode 15.
    Word2013,
}

/// A legacy compatibility option of a document: a child of `w:compat` (ECMA-376 Part 1,
/// §17.15.3), or a `w:compatSetting` set to true. The options the layout follows have a
/// variant each; every other one is [`Other`](Self::Other), and the layout reports it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CompatFlag {
    /// `doNotExpandShiftReturn`: a justified line that ends in a line break is not
    /// stretched.
    DoNotExpandShiftReturn,
    /// `doNotUseIndentAsNumberingTabStop`: a hanging indent is not a tab stop for the
    /// label of a list item.
    DoNotUseIndentAsNumberingTabStop,
    /// `doNotUseHTMLParagraphAutoSpacing`: automatic spacing before and after paragraphs
    /// uses Word's values instead of HTML's.
    DoNotUseHTMLParagraphAutoSpacing,
    /// `noLeading`: lines have no external leading.
    NoLeading,
    /// `splitPgBreakAndParaMark`: a page break at the end of a paragraph does not move the
    /// paragraph mark to the next page.
    SplitPgBreakAndParaMark,
    /// `ulTrailSpace`: the spaces at the end of a line are underlined.
    UlTrailSpace,
    /// `growAutofit`: autofit tables may grow into the margins.
    GrowAutofit,
    /// `doNotAutofitConstrainedTables`: autofit does not apply to tables whose preferred
    /// widths do not fit.
    DoNotAutofitConstrainedTables,
    /// `layoutRawTableWidth`: table widths ignore the cell spacing.
    LayoutRawTableWidth,
    /// `layoutTableRowsApart`: rows of a table may be split from each other freely.
    LayoutTableRowsApart,
    /// `allowSpaceOfSameStyleInTable`: contextual spacing does not apply inside tables.
    AllowSpaceOfSameStyleInTable,
    /// `doNotVertAlignCellWithSp`: cells holding a floating object are not aligned
    /// vertically.
    DoNotVertAlignCellWithSp,
    /// `doNotBreakWrappedTables`: floating tables do not break across pages.
    DoNotBreakWrappedTables,
    /// Any other option, by its element or setting name, such as `usePrinterMetrics`.
    Other(String),
}

impl CompatFlag {
    /// Every variant but [`Other`](Self::Other), for [`from_name`](Self::from_name).
    pub(super) const NAMED: [CompatFlag; 13] = [
        Self::DoNotExpandShiftReturn,
        Self::DoNotUseIndentAsNumberingTabStop,
        Self::DoNotUseHTMLParagraphAutoSpacing,
        Self::NoLeading,
        Self::SplitPgBreakAndParaMark,
        Self::UlTrailSpace,
        Self::GrowAutofit,
        Self::DoNotAutofitConstrainedTables,
        Self::LayoutRawTableWidth,
        Self::LayoutTableRowsApart,
        Self::AllowSpaceOfSameStyleInTable,
        Self::DoNotVertAlignCellWithSp,
        Self::DoNotBreakWrappedTables,
    ];

    /// The option with this element or setting name, as WordprocessingML writes it.
    /// Names the layout does not follow become [`Other`](Self::Other), so a producer can
    /// pass every option it reads.
    ///
    /// ```
    /// use officewright_layout::ir::CompatFlag;
    ///
    /// assert_eq!(CompatFlag::from_name("noLeading"), CompatFlag::NoLeading);
    /// assert_eq!(
    ///     CompatFlag::from_name("usePrinterMetrics"),
    ///     CompatFlag::Other("usePrinterMetrics".into()),
    /// );
    /// ```
    pub fn from_name(name: &str) -> Self {
        Self::NAMED
            .into_iter()
            .find(|flag| flag.name() == name)
            .unwrap_or_else(|| Self::Other(name.to_owned()))
    }

    /// The element or setting name of the option.
    pub fn name(&self) -> &str {
        match self {
            Self::DoNotExpandShiftReturn => "doNotExpandShiftReturn",
            Self::DoNotUseIndentAsNumberingTabStop => "doNotUseIndentAsNumberingTabStop",
            Self::DoNotUseHTMLParagraphAutoSpacing => "doNotUseHTMLParagraphAutoSpacing",
            Self::NoLeading => "noLeading",
            Self::SplitPgBreakAndParaMark => "splitPgBreakAndParaMark",
            Self::UlTrailSpace => "ulTrailSpace",
            Self::GrowAutofit => "growAutofit",
            Self::DoNotAutofitConstrainedTables => "doNotAutofitConstrainedTables",
            Self::LayoutRawTableWidth => "layoutRawTableWidth",
            Self::LayoutTableRowsApart => "layoutTableRowsApart",
            Self::AllowSpaceOfSameStyleInTable => "allowSpaceOfSameStyleInTable",
            Self::DoNotVertAlignCellWithSp => "doNotVertAlignCellWithSp",
            Self::DoNotBreakWrappedTables => "doNotBreakWrappedTables",
            Self::Other(name) => name,
        }
    }
}

ir_struct! {
    /// Automatic hyphenation of a document (ECMA-376 Part 1, §17.15.1.10 and following).
    /// Not laid out yet: a document that turns it on is reported.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Hyphenation {
        /// Whether words are hyphenated automatically.
        copy auto: bool = false => with_auto;
        /// How far from the end edge a line may end before a word is hyphenated. ECMA-376
        /// gives 18 pt when a document does not say; Word picks a value by the language
        /// of the system (MS-OI29500 2.1.407 a).
        copy zone: Pt = Pt(18.0) => with_zone, check rules::non_negative;
        /// The most lines in a row that may end in a hyphen; 0 means no limit.
        copy consecutive_limit: u32 = 0 => with_consecutive_limit;
        /// Whether words in capital letters are hyphenated.
        copy hyphenate_caps: bool = true => with_hyphenate_caps;
    }
}

no_rules!(CompatMode, CompatFlag);
