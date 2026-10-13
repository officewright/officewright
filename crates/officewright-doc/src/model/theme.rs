//! The theme of a document (`theme1.xml`, DrawingML, ECMA-376 Part 1, §20.1.6), as
//! written: the colour scheme and the font scheme that theme colours and theme fonts
//! stand for.

use super::properties::xml_enum;
use super::settings::ColorSchemeIndex;

/// The theme of a document: its colour scheme and font scheme. The rest of the part,
/// such as the format scheme of shapes, is kept for writing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Theme {
    pub(crate) name: Option<Box<str>>,
    pub(crate) colors: Option<ColorScheme>,
    pub(crate) fonts: Option<FontScheme>,
}

impl Theme {
    /// The theme's name (`name` of `a:theme`).
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The colour scheme (`a:clrScheme`).
    pub fn colors(&self) -> Option<&ColorScheme> {
        self.colors.as_ref()
    }

    /// The font scheme (`a:fontScheme`).
    pub fn fonts(&self) -> Option<&FontScheme> {
        self.fonts.as_ref()
    }
}

/// The twelve colours of a theme (`a:clrScheme`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ColorScheme {
    pub(crate) name: Option<Box<str>>,
    /// In the order of [`ColorSchemeIndex`]: dark 1, light 1, dark 2, light 2, the six
    /// accents, hyperlink, followed hyperlink.
    pub(crate) colors: [Option<SchemeColor>; 12],
}

impl ColorScheme {
    /// The scheme's name.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// One colour of the scheme; `None` when it is missing, not valid, or written in a
    /// colour model this reader does not read yet (only RGB and system colours are,
    /// the two Word writes).
    pub fn color(&self, index: ColorSchemeIndex) -> Option<&SchemeColor> {
        self.colors.get(position(index))?.as_ref()
    }
}

/// A colour of a theme's scheme. Colour transforms written on it (tints, shades and the
/// like) are not read yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SchemeColor {
    /// Red, green and blue (`a:srgbClr`).
    Rgb(u8, u8, u8),
    /// A colour of the operating system (`a:sysClr`), and the value it had where the
    /// document was last saved (`lastClr`).
    System {
        /// The system colour.
        color: SystemColor,
        /// Its value when the document was saved.
        last: Option<(u8, u8, u8)>,
    },
}

impl SchemeColor {
    /// The colour as red, green and blue: for a system colour, the value it had where
    /// the document was saved.
    pub fn rgb(&self) -> Option<(u8, u8, u8)> {
        match *self {
            Self::Rgb(red, green, blue) => Some((red, green, blue)),
            Self::System { last, .. } => last,
        }
    }
}

xml_enum! {
    /// A colour of the operating system (ST_SystemColorVal).
    SystemColor {
        ScrollBar = "scrollBar",
        Background = "background",
        ActiveCaption = "activeCaption",
        InactiveCaption = "inactiveCaption",
        Menu = "menu",
        Window = "window",
        WindowFrame = "windowFrame",
        MenuText = "menuText",
        WindowText = "windowText",
        CaptionText = "captionText",
        ActiveBorder = "activeBorder",
        InactiveBorder = "inactiveBorder",
        AppWorkspace = "appWorkspace",
        Highlight = "highlight",
        HighlightText = "highlightText",
        ButtonFace = "btnFace",
        ButtonShadow = "btnShadow",
        GrayText = "grayText",
        ButtonText = "btnText",
        InactiveCaptionText = "inactiveCaptionText",
        ButtonHighlight = "btnHighlight",
        ThreeDDarkShadow = "3dDkShadow",
        ThreeDLight = "3dLight",
        InfoText = "infoText",
        InfoBackground = "infoBk",
        HotLight = "hotLight",
        GradientActiveCaption = "gradientActiveCaption",
        GradientInactiveCaption = "gradientInactiveCaption",
        MenuHighlight = "menuHighlight",
        MenuBar = "menuBar",
    }
}

/// The fonts of a theme (`a:fontScheme`): one set for headings, one for body text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct FontScheme {
    pub(crate) name: Option<Box<str>>,
    pub(crate) major: FontCollection,
    pub(crate) minor: FontCollection,
}

impl FontScheme {
    /// The scheme's name.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The fonts of headings (`a:majorFont`), which the theme fonts `major…` name.
    pub fn major(&self) -> &FontCollection {
        &self.major
    }

    /// The fonts of body text (`a:minorFont`), which the theme fonts `minor…` name.
    pub fn minor(&self) -> &FontCollection {
        &self.minor
    }
}

/// One set of theme fonts (`a:majorFont`, `a:minorFont`): a font for Latin, East Asian
/// and complex-script text, and fonts for particular scripts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct FontCollection {
    pub(crate) latin: Option<Box<str>>,
    pub(crate) east_asian: Option<Box<str>>,
    pub(crate) complex_script: Option<Box<str>>,
    pub(crate) scripts: Vec<(Box<str>, Box<str>)>,
}

impl FontCollection {
    /// The font of Latin text (`a:latin`).
    pub fn latin(&self) -> Option<&str> {
        self.latin.as_deref()
    }

    /// The font of East Asian text (`a:ea`); often empty, which leaves the choice to a
    /// font for the script.
    pub fn east_asian(&self) -> Option<&str> {
        self.east_asian.as_deref()
    }

    /// The font of complex-script text (`a:cs`).
    pub fn complex_script(&self) -> Option<&str> {
        self.complex_script.as_deref()
    }

    /// The font for a script (`a:font`), named by its ISO 15924 code as the theme
    /// writes it, such as `Jpan` or `Cyrl`.
    pub fn script(&self, script: &str) -> Option<&str> {
        self.scripts
            .iter()
            .find(|(written, _)| &**written == script)
            .map(|(_, typeface)| &**typeface)
    }

    /// Every font for a script, as (script, typeface), in document order.
    pub fn scripts(&self) -> impl ExactSizeIterator<Item = (&str, &str)> + '_ {
        self.scripts
            .iter()
            .map(|(script, typeface)| (&**script, &**typeface))
    }
}

/// Where a colour of the scheme is kept in [`ColorScheme::colors`].
pub(crate) fn position(index: ColorSchemeIndex) -> usize {
    match index {
        ColorSchemeIndex::Dark1 => 0,
        ColorSchemeIndex::Light1 => 1,
        ColorSchemeIndex::Dark2 => 2,
        ColorSchemeIndex::Light2 => 3,
        ColorSchemeIndex::Accent1 => 4,
        ColorSchemeIndex::Accent2 => 5,
        ColorSchemeIndex::Accent3 => 6,
        ColorSchemeIndex::Accent4 => 7,
        ColorSchemeIndex::Accent5 => 8,
        ColorSchemeIndex::Accent6 => 9,
        ColorSchemeIndex::Hyperlink => 10,
        ColorSchemeIndex::FollowedHyperlink => 11,
    }
}
