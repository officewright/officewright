//! Entry points for the fuzz targets in `fuzz/`, the examples and the tests that use the
//! repository's test fonts: the layout's own code, reached without a whole document.
//! They are not part of the API and change with the code they reach.

use officewright_font::FontFace;

use crate::ir::Ligatures;
use crate::shape::{Features, ShapeRequest, Shaper, ShapingFace};

/// A glyph as [`shape`] places it, in the face's design units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glyph {
    id: u16,
    cluster: usize,
    x_advance: i32,
    x_offset: i32,
    y_offset: i32,
    unsafe_to_break: bool,
}

impl Glyph {
    /// The glyph's id in the face.
    pub fn id(self) -> u16 {
        self.id
    }

    /// The byte offset in the text of the first character the glyph stands for.
    pub fn cluster(self) -> usize {
        self.cluster
    }

    /// How far the glyph moves the pen.
    pub fn x_advance(self) -> i32 {
        self.x_advance
    }

    /// How far the glyph is drawn right of the pen.
    pub fn x_offset(self) -> i32 {
        self.x_offset
    }

    /// How far the glyph is drawn above the pen.
    pub fn y_offset(self) -> i32 {
        self.y_offset
    }

    /// Whether breaking the text at this glyph's cluster could change the shaping.
    pub fn unsafe_to_break(self) -> bool {
        self.unsafe_to_break
    }
}

/// The result of [`shape`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shaped {
    glyphs: Vec<Glyph>,
    nominal: bool,
    replaced: usize,
}

impl Shaped {
    /// The glyphs, left to right.
    pub fn glyphs(&self) -> &[Glyph] {
        &self.glyphs
    }

    /// Whether the text was shaped from the character map alone, because the face's
    /// layout tables could not be read.
    pub fn is_nominal(&self) -> bool {
        self.nominal
    }

    /// How many glyphs the face's layout tables named that the face does not have,
    /// replaced by the missing-glyph shape.
    pub fn replaced_glyphs(&self) -> usize {
        self.replaced
    }
}

/// Shapes `text` in `face`, left to right, as the layout shapes a run: kerning and the
/// ligatures as given, the script taken from the text, in a language given as a BCP 47
/// tag.
pub fn shape(
    face: &FontFace,
    text: &str,
    kerning: bool,
    ligatures: Ligatures,
    language: Option<&str>,
) -> Shaped {
    let face = ShapingFace::new(face.clone());
    let request = ShapeRequest {
        features: Features { kerning, ligatures },
        script: None,
        language: language.map(str::to_owned),
    };
    let run = Shaper::new().shape(&face, text, &request);
    Shaped {
        glyphs: run
            .glyphs()
            .iter()
            .map(|glyph| Glyph {
                id: glyph.glyph.to_u16(),
                cluster: glyph.cluster,
                x_advance: glyph.x_advance,
                x_offset: glyph.x_offset,
                y_offset: glyph.y_offset,
                unsafe_to_break: glyph.unsafe_to_break,
            })
            .collect(),
        nominal: run.is_nominal(),
        replaced: run.replaced_glyphs(),
    }
}

/// Shapes each text in `face` with kerning off and on and with no ligatures and all of
/// them, and checks what the layout relies on: every glyph is one the face has, and the
/// clusters are character boundaries of the text that never go backwards.
///
/// # Errors
///
/// What was wrong, naming the text, the features and the glyph.
pub fn check_shaping(face: &FontFace, texts: &[&str]) -> Result<(), String> {
    let shaping_face = ShapingFace::new(face.clone());
    let mut shaper = Shaper::new();
    let every = Ligatures::default()
        .with_standard(true)
        .with_contextual(true)
        .with_historical(true)
        .with_discretional(true);
    for text in texts {
        for kerning in [false, true] {
            for ligatures in [Ligatures::default(), every] {
                let request = ShapeRequest {
                    features: Features { kerning, ligatures },
                    script: None,
                    language: None,
                };
                let run = shaper.shape(&shaping_face, text, &request);
                let mut previous = 0;
                for (index, glyph) in run.glyphs().iter().enumerate() {
                    let at = || format!("{text:?} with {request:?}, glyph {index}");
                    if glyph.glyph.to_u16() >= face.glyph_count() {
                        return Err(format!(
                            "{}: glyph {} does not exist",
                            at(),
                            glyph.glyph.to_u16()
                        ));
                    }
                    if !text.is_char_boundary(glyph.cluster) {
                        return Err(format!(
                            "{}: cluster {} is not a character boundary",
                            at(),
                            glyph.cluster
                        ));
                    }
                    if glyph.cluster < previous {
                        return Err(format!(
                            "{}: cluster {} goes backwards",
                            at(),
                            glyph.cluster
                        ));
                    }
                    previous = glyph.cluster;
                }
            }
        }
    }
    Ok(())
}
