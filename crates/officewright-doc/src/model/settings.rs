//! The settings of a document (`settings.xml`, ECMA-376 Part 1, §17.15), as written.

use officewright_core::Pt;

use super::properties::{Language, xml_enum};
use super::section::NoteProperties;

/// The feature set of the Word version a document was last saved for (the
/// `compatibilityMode` setting, [MS-DOCX] 2.3.5): it changes how Word lays out
/// the document. An open set of values, so a later version's mode breaks nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CompatibilityMode(u32);

impl CompatibilityMode {
    /// Word 2003: the features of the binary format.
    pub const WORD_2003: Self = Self(11);
    /// Word 2007: the features of ECMA-376, first edition. The mode of a document that
    /// does not give one ([MS-DOCX] 2.3.5).
    pub const WORD_2007: Self = Self(12);
    /// Word 2010.
    pub const WORD_2010: Self = Self(14);
    /// Word 2013 and later.
    pub const WORD_2013: Self = Self(15);

    /// The mode with this number.
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// The number of the mode, as written.
    pub const fn value(self) -> u32 {
        self.0
    }
}

xml_enum! {
    /// A legacy compatibility option (a child of `w:compat`, ECMA-376 Part 1, §17.15.3):
    /// each makes Word lay out or behave as an older version, or another word
    /// processor, did.
    CompatibilityOption {
        UseSingleBorderforContiguousCells = "useSingleBorderforContiguousCells",
        WpJustification = "wpJustification",
        NoTabHangInd = "noTabHangInd",
        NoLeading = "noLeading",
        SpaceForUL = "spaceForUL",
        NoColumnBalance = "noColumnBalance",
        BalanceSingleByteDoubleByteWidth = "balanceSingleByteDoubleByteWidth",
        NoExtraLineSpacing = "noExtraLineSpacing",
        DoNotLeaveBackslashAlone = "doNotLeaveBackslashAlone",
        UlTrailSpace = "ulTrailSpace",
        DoNotExpandShiftReturn = "doNotExpandShiftReturn",
        SpacingInWholePoints = "spacingInWholePoints",
        LineWrapLikeWord6 = "lineWrapLikeWord6",
        PrintBodyTextBeforeHeader = "printBodyTextBeforeHeader",
        PrintColBlack = "printColBlack",
        WpSpaceWidth = "wpSpaceWidth",
        ShowBreaksInFrames = "showBreaksInFrames",
        SubFontBySize = "subFontBySize",
        SuppressBottomSpacing = "suppressBottomSpacing",
        SuppressTopSpacing = "suppressTopSpacing",
        SuppressSpacingAtTopOfPage = "suppressSpacingAtTopOfPage",
        SuppressTopSpacingWP = "suppressTopSpacingWP",
        SuppressSpBfAfterPgBrk = "suppressSpBfAfterPgBrk",
        SwapBordersFacingPages = "swapBordersFacingPages",
        ConvMailMergeEsc = "convMailMergeEsc",
        TruncateFontHeightsLikeWP6 = "truncateFontHeightsLikeWP6",
        MwSmallCaps = "mwSmallCaps",
        UsePrinterMetrics = "usePrinterMetrics",
        DoNotSuppressParagraphBorders = "doNotSuppressParagraphBorders",
        WrapTrailSpaces = "wrapTrailSpaces",
        FootnoteLayoutLikeWW8 = "footnoteLayoutLikeWW8",
        ShapeLayoutLikeWW8 = "shapeLayoutLikeWW8",
        AlignTablesRowByRow = "alignTablesRowByRow",
        ForgetLastTabAlignment = "forgetLastTabAlignment",
        AdjustLineHeightInTable = "adjustLineHeightInTable",
        AutoSpaceLikeWord95 = "autoSpaceLikeWord95",
        NoSpaceRaiseLower = "noSpaceRaiseLower",
        DoNotUseHTMLParagraphAutoSpacing = "doNotUseHTMLParagraphAutoSpacing",
        LayoutRawTableWidth = "layoutRawTableWidth",
        LayoutTableRowsApart = "layoutTableRowsApart",
        UseWord97LineBreakRules = "useWord97LineBreakRules",
        DoNotBreakWrappedTables = "doNotBreakWrappedTables",
        DoNotSnapToGridInCell = "doNotSnapToGridInCell",
        SelectFldWithFirstOrLastChar = "selectFldWithFirstOrLastChar",
        ApplyBreakingRules = "applyBreakingRules",
        DoNotWrapTextWithPunct = "doNotWrapTextWithPunct",
        DoNotUseEastAsianBreakRules = "doNotUseEastAsianBreakRules",
        UseWord2002TableStyleRules = "useWord2002TableStyleRules",
        GrowAutofit = "growAutofit",
        UseFELayout = "useFELayout",
        UseNormalStyleForList = "useNormalStyleForList",
        DoNotUseIndentAsNumberingTabStop = "doNotUseIndentAsNumberingTabStop",
        UseAltKinsokuLineBreakRules = "useAltKinsokuLineBreakRules",
        AllowSpaceOfSameStyleInTable = "allowSpaceOfSameStyleInTable",
        DoNotSuppressIndentation = "doNotSuppressIndentation",
        DoNotAutofitConstrainedTables = "doNotAutofitConstrainedTables",
        AutofitToFirstFixedWidthCell = "autofitToFirstFixedWidthCell",
        UnderlineTabInNumList = "underlineTabInNumList",
        DisplayHangulFixedWidth = "displayHangulFixedWidth",
        SplitPgBreakAndParaMark = "splitPgBreakAndParaMark",
        DoNotVertAlignCellWithSp = "doNotVertAlignCellWithSp",
        DoNotBreakConstrainedForcedTable = "doNotBreakConstrainedForcedTable",
        DoNotVertAlignInTxbx = "doNotVertAlignInTxbx",
        UseAnsiKerningPairs = "useAnsiKerningPairs",
        CachedColBalance = "cachedColBalance",
    }
}

/// A compatibility setting as written (`w:compatSetting`): a name, the namespace that
/// defines it and its value.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CompatibilitySetting {
    pub(crate) name: Box<str>,
    pub(crate) uri: Option<Box<str>>,
    pub(crate) value: Option<Box<str>>,
}

impl CompatibilitySetting {
    /// The name (`w:name`), such as `compatibilityMode`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The namespace that defines it (`w:uri`).
    pub fn uri(&self) -> Option<&str> {
        self.uri.as_deref()
    }

    /// The value (`w:val`).
    pub fn value(&self) -> Option<&str> {
        self.value.as_deref()
    }
}

xml_enum! {
    /// A colour of a theme's colour scheme (ST_WmlColorSchemeIndex).
    ColorSchemeIndex {
        Dark1 = "dark1",
        Light1 = "light1",
        Dark2 = "dark2",
        Light2 = "light2",
        Accent1 = "accent1",
        Accent2 = "accent2",
        Accent3 = "accent3",
        Accent4 = "accent4",
        Accent5 = "accent5",
        Accent6 = "accent6",
        Hyperlink = "hyperlink",
        FollowedHyperlink = "followedHyperlink",
    }
}

/// Which colours of the theme's scheme the document's theme colours stand for
/// (`w:clrSchemeMapping`): text 1, for one, is usually dark 1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ColorSchemeMapping {
    pub(crate) background1: Option<ColorSchemeIndex>,
    pub(crate) text1: Option<ColorSchemeIndex>,
    pub(crate) background2: Option<ColorSchemeIndex>,
    pub(crate) text2: Option<ColorSchemeIndex>,
    pub(crate) accents: [Option<ColorSchemeIndex>; 6],
    pub(crate) hyperlink: Option<ColorSchemeIndex>,
    pub(crate) followed_hyperlink: Option<ColorSchemeIndex>,
}

impl ColorSchemeMapping {
    /// What background 1 stands for (`w:bg1`).
    pub fn background1(&self) -> Option<ColorSchemeIndex> {
        self.background1
    }

    /// What text 1 stands for (`w:t1`).
    pub fn text1(&self) -> Option<ColorSchemeIndex> {
        self.text1
    }

    /// What background 2 stands for (`w:bg2`).
    pub fn background2(&self) -> Option<ColorSchemeIndex> {
        self.background2
    }

    /// What text 2 stands for (`w:t2`).
    pub fn text2(&self) -> Option<ColorSchemeIndex> {
        self.text2
    }

    /// What accent 1 to 6 stands for (`w:accent1` to `w:accent6`); `None` for a number
    /// outside 1 to 6.
    pub fn accent(&self, number: usize) -> Option<ColorSchemeIndex> {
        self.accents.get(number.checked_sub(1)?).copied().flatten()
    }

    /// What the hyperlink colour stands for (`w:hyperlink`).
    pub fn hyperlink(&self) -> Option<ColorSchemeIndex> {
        self.hyperlink
    }

    /// What the followed hyperlink colour stands for (`w:followedHyperlink`).
    pub fn followed_hyperlink(&self) -> Option<ColorSchemeIndex> {
        self.followed_hyperlink
    }
}

/// The settings of a document that bear on its layout (`settings.xml`), as written. A
/// setting the document does not give is `None`. The rest of the part is kept for
/// writing.
///
/// ```
/// # fn main() -> officewright_doc::Result<()> {
/// use officewright_doc::{CompatibilityMode, Document};
///
/// let document = Document::open("tests/data/hello.docx")?;
/// let settings = document.settings();
/// // The file has no settings part, so Word lays it out in the mode of Word 2007.
/// assert_eq!(settings.compatibility_mode(), None);
/// assert_eq!(settings.effective_compatibility_mode(), CompatibilityMode::WORD_2007);
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Settings {
    pub(crate) compatibility_mode: Option<CompatibilityMode>,
    pub(crate) compatibility_settings: Vec<CompatibilitySetting>,
    pub(crate) compatibility_options: Vec<(CompatibilityOption, bool)>,
    pub(crate) default_tab_stop: Option<Pt>,
    pub(crate) even_and_odd_headers: Option<bool>,
    pub(crate) mirror_margins: Option<bool>,
    pub(crate) gutter_at_top: Option<bool>,
    pub(crate) borders_do_not_surround_header: Option<bool>,
    pub(crate) borders_do_not_surround_footer: Option<bool>,
    pub(crate) auto_hyphenation: Option<bool>,
    pub(crate) consecutive_hyphen_limit: Option<i64>,
    pub(crate) hyphenation_zone: Option<Pt>,
    pub(crate) do_not_hyphenate_caps: Option<bool>,
    pub(crate) decimal_symbol: Option<Box<str>>,
    pub(crate) list_separator: Option<Box<str>>,
    pub(crate) default_table_style: Option<Box<str>>,
    pub(crate) footnotes: Option<NoteProperties>,
    pub(crate) endnotes: Option<NoteProperties>,
    pub(crate) footnote_separators: Vec<i64>,
    pub(crate) endnote_separators: Vec<i64>,
    pub(crate) theme_font_languages: Option<Language>,
    pub(crate) color_scheme_mapping: Option<ColorSchemeMapping>,
}

impl Settings {
    /// The compatibility mode as written (the `w:compatSetting` named
    /// `compatibilityMode` in Word's namespace).
    pub fn compatibility_mode(&self) -> Option<CompatibilityMode> {
        self.compatibility_mode
    }

    /// The compatibility mode Word lays the document out in: the one written, or
    /// [`CompatibilityMode::WORD_2007`] when there is none ([MS-DOCX] 2.3.5).
    pub fn effective_compatibility_mode(&self) -> CompatibilityMode {
        self.compatibility_mode
            .unwrap_or(CompatibilityMode::WORD_2007)
    }

    /// Every compatibility setting written (`w:compatSetting`), the mode among them.
    pub fn compatibility_settings(&self) -> &[CompatibilitySetting] {
        &self.compatibility_settings
    }

    /// Whether a legacy compatibility option is set, as written; the last one written
    /// counts.
    pub fn compatibility_option(&self, option: CompatibilityOption) -> Option<bool> {
        self.compatibility_options
            .iter()
            .rev()
            .find(|(written, _)| *written == option)
            .map(|&(_, on)| on)
    }

    /// The legacy compatibility options written, in document order.
    pub fn compatibility_options(
        &self,
    ) -> impl ExactSizeIterator<Item = (CompatibilityOption, bool)> + '_ {
        self.compatibility_options.iter().copied()
    }

    /// The distance between default tab stops (`w:defaultTabStop`).
    pub fn default_tab_stop(&self) -> Option<Pt> {
        self.default_tab_stop
    }

    /// Odd and even pages have different headers and footers (`w:evenAndOddHeaders`).
    pub fn even_and_odd_headers(&self) -> Option<bool> {
        self.even_and_odd_headers
    }

    /// Facing pages swap their left and right margins (`w:mirrorMargins`).
    pub fn mirror_margins(&self) -> Option<bool> {
        self.mirror_margins
    }

    /// The gutter is at the top of the page (`w:gutterAtTop`).
    pub fn gutter_at_top(&self) -> Option<bool> {
        self.gutter_at_top
    }

    /// Page borders leave out the header (`w:bordersDoNotSurroundHeader`).
    pub fn borders_do_not_surround_header(&self) -> Option<bool> {
        self.borders_do_not_surround_header
    }

    /// Page borders leave out the footer (`w:bordersDoNotSurroundFooter`).
    pub fn borders_do_not_surround_footer(&self) -> Option<bool> {
        self.borders_do_not_surround_footer
    }

    /// Words are hyphenated automatically (`w:autoHyphenation`).
    pub fn auto_hyphenation(&self) -> Option<bool> {
        self.auto_hyphenation
    }

    /// The most lines in a row that may end with a hyphen (`w:consecutiveHyphenLimit`).
    pub fn consecutive_hyphen_limit(&self) -> Option<i64> {
        self.consecutive_hyphen_limit
    }

    /// How far from the end margin a word may be hyphenated (`w:hyphenationZone`).
    pub fn hyphenation_zone(&self) -> Option<Pt> {
        self.hyphenation_zone
    }

    /// Words in capitals are not hyphenated (`w:doNotHyphenateCaps`).
    pub fn do_not_hyphenate_caps(&self) -> Option<bool> {
        self.do_not_hyphenate_caps
    }

    /// The decimal separator that decimal tab stops align on (`w:decimalSymbol`).
    pub fn decimal_symbol(&self) -> Option<&str> {
        self.decimal_symbol.as_deref()
    }

    /// The list separator of fields (`w:listSeparator`).
    pub fn list_separator(&self) -> Option<&str> {
        self.list_separator.as_deref()
    }

    /// The style id new tables get (`w:defaultTableStyle`).
    pub fn default_table_style(&self) -> Option<&str> {
        self.default_table_style.as_deref()
    }

    /// The document's footnote placement and numbering (`w:footnotePr`), which
    /// sections may change.
    pub fn footnotes(&self) -> Option<&NoteProperties> {
        self.footnotes.as_ref()
    }

    /// The document's endnote placement and numbering (`w:endnotePr`).
    pub fn endnotes(&self) -> Option<&NoteProperties> {
        self.endnotes.as_ref()
    }

    /// The ids of the notes that separate footnotes from the text (`w:footnote` in
    /// `w:footnotePr`).
    pub fn footnote_separators(&self) -> &[i64] {
        &self.footnote_separators
    }

    /// The ids of the notes that separate endnotes from the text (`w:endnote` in
    /// `w:endnotePr`).
    pub fn endnote_separators(&self) -> &[i64] {
        &self.endnote_separators
    }

    /// The languages whose fonts the theme's fonts stand for (`w:themeFontLang`).
    pub fn theme_font_languages(&self) -> Option<&Language> {
        self.theme_font_languages.as_ref()
    }

    /// Which colours of the theme the document's theme colours stand for
    /// (`w:clrSchemeMapping`).
    pub fn color_scheme_mapping(&self) -> Option<&ColorSchemeMapping> {
        self.color_scheme_mapping.as_ref()
    }
}
