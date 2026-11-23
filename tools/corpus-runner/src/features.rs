//! Which entries of the layout's report degrade a required feature, for gate row 12. The
//! required features are those the release criterion of Milestone 4 names: all of phase 1
//! of the plan and, of phase 2, footnotes and endnotes, picture cropping and rotation by
//! 90 degrees, basic text boxes and hyperlinks. The rest of phase 2 and what the plan
//! leaves for later degrade with an entry and are not required; nor is what phase 1 lays
//! out otherwise on purpose and reports, such as floating tables laid out in the text.
//!
//! An entry is told by its kind, `feature-unsupported`, and by words of its message. An
//! entry of that kind that no line below names is counted as degrading a required feature
//! until a line classes it, so that a new or reworded message cannot pass unseen. Every
//! message the layout and the translation into its IR wrote when these lines were made is
//! classed, but for those that name markup the translation does not know ("a block-level
//! w:… is left out", "an inline … element is left out", "an element in a run is left out")
//! and the note that more entries are not listed: text may be lost there, so they stay
//! unclassed on purpose.

/// What an entry of the layout's report does to the required features.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// It degrades the required feature of this name.
    Required(&'static str),
    /// It concerns no required feature.
    NotRequired,
    /// No line of [`MESSAGES`] names it; it counts as degrading a required feature.
    Unclassified,
}

/// Words of the messages of `feature-unsupported` entries, each with the required feature
/// it degrades, or `None` for an entry that degrades none. The first line whose words the
/// message contains classes it.
pub const MESSAGES: [(&str, Option<&str>); 123] = [
    // Paragraphs: borders and shading (phase 1).
    ("paragraph borders are not drawn", Some("paragraph borders")),
    ("a shading pattern of stripes or crosses", Some("shading")),
    // Footnotes and endnotes (phase 2, required).
    ("endnotes are not laid out", Some("notes")),
    ("footnotes referenced in table cells", Some("notes")),
    ("note numbers that start again on each page", Some("notes")),
    ("note references in headers and footers", Some("notes")),
    ("the rest of a footnote cut on a page", Some("notes")),
    // Pictures: cropping and rotation (phase 2, required), PNG and JPEG anchored or inline
    // (phase 1), also in table cells.
    ("cropped, turned or mirrored pictures", Some("pictures")),
    ("a picture is drawn as a grey box:", Some("pictures")),
    (
        "floating objects in table cells are not drawn",
        Some("pictures"),
    ),
    // Tables (phase 1).
    (
        "rows that a vertically merged cell ties together",
        Some("tables"),
    ),
    ("the row's table property exceptions", Some("tables")),
    ("the row's own alignment", Some("tables")),
    ("the row's own cell spacing", Some("tables")),
    ("the row's hidden end mark", Some("tables")),
    ("a cell that continues a horizontal merge", Some("tables")),
    (
        "the table style's conditional formats are not applied",
        Some("tables"),
    ),
    // A limit of the layout, before the line it would match below.
    ("table header rows are not repeated past", None),
    ("table header rows are not repeated", Some("tables")),
    ("a page field in table header rows", Some("tables")),
    ("a page field in a table row cut over pages", Some("tables")),
    (
        "header rows of a table in a table cell are not repeated",
        Some("tables"),
    ),
    (
        "spacing between table cells is laid out as none",
        Some("tables"),
    ),
    // Fields: PAGE, NUMPAGES and SECTIONPAGES (phase 1), and the formats of numbers;
    // list and note numbers in words first, before the line that matches them all.
    (
        "list numbers in words and ordinals in languages",
        Some("lists"),
    ),
    (
        "note numbers in words and ordinals in languages",
        Some("notes"),
    ),
    ("numbers in words and ordinals in languages", Some("fields")),
    (
        "a page field in the body text shows the number",
        Some("fields"),
    ),
    (
        "a page field of a paragraph split over pages",
        Some("fields"),
    ),
    ("the number of pages changed in each of", Some("fields")),
    ("a continuation notice that does not fit", Some("notes")),
    // Basic text boxes (decision 15): their paragraphs, inside the box.
    ("a vml picture's text", Some("text boxes")),
    (
        "text taller than a text box that cuts it off",
        Some("text boxes"),
    ),
    // Not required: what the plan leaves for later, or for after the first release.
    ("text does not wrap around floating objects", None),
    ("text in columns is laid out in one column", None),
    ("a continuous section with another page size", None),
    ("the document grid", None),
    ("spacing in line units on a document grid", None),
    ("vertical text", None),
    ("the body text of a section is laid out from the top", None),
    ("automatic hyphenation", None),
    ("the cell's text fitting", None),
    ("format are drawn as grey boxes", None),
    ("a drawing that shows no picture", None),
    ("a chart is left out", None),
    ("a vml drawing", None),
    ("a group of vml shapes", None),
    ("an embedded object", None),
    ("an equation is left out", None),
    ("a text box shaped other than as a rectangle", None),
    ("tables in text boxes", None),
    ("floating objects in text boxes", None),
    ("a chain of linked text boxes", None),
    ("a turned text box", None),
    ("the columns of a text box", None),
    ("wordart", None),
    ("line numbers (w:lnnumtype)", None),
    ("page borders (w:pgborders)", None),
    ("a right-to-left paragraph", None),
    ("a drop cap (w:framepr)", None),
    ("right-to-left text is laid out", None),
    ("right-to-left tables", None),
    // Not required: tracked changes are laid out as accepted (plan 1.3).
    ("as a tracked change", None),
    // Not required: effects on text and pictures the plan does not name.
    ("3-d text", None),
    ("a fill of text", None),
    ("an outline of text", None),
    ("a reflection of text", None),
    ("glowing text", None),
    ("text with a shadow effect", None),
    ("embossed text", None),
    ("engraved text", None),
    ("outlined text", None),
    ("shadowed text", None),
    ("emphasis marks", None),
    ("borders around text", None),
    ("stylistic sets", None),
    ("lining or old-style figures", None),
    ("proportional or tabular figures", None),
    ("a picture cut to a shape", None),
    (
        "a picture or an object in the line of a raised or lowered run",
        None,
    ),
    // Not required: laid out otherwise on purpose in phase 1, and reported.
    ("floating tables are laid out in the text", None),
    (
        "a text frame (w:framepr) is laid out as an ordinary paragraph",
        None,
    ),
    // Not required: details the plan does not name.
    ("an absolute position tab", None),
    ("a picture's ", None),
    ("a vml picture's ", None),
    ("a text box's ", None),
    ("the cell's diagonal borders", None),
    ("list numbers in a format the layout does not write", None),
    ("numbers in a format the layout does not write", None),
    ("numbers that their format cannot represent", None),
    // Markup Word itself never writes.
    ("the content of a cell merged into the cell above it", None),
    ("a picture bullet is left out", None),
    ("chapter numbers before page numbers", None),
    ("a page number (w:pgnum) inside a field", None),
    ("is laid out as justified", None),
    // Not required: limits, and markup that is damaged or that Word does not make.
    ("bar tabs of a paragraph are drawn", None),
    ("a tab leader that would take more than", None),
    (
        "list labels, and the templates they are written from, longer than",
        None,
    ),
    ("pictures larger than", None),
    ("a picture's image is not in the package", None),
    ("a picture without a size", None),
    ("a text box without a size", None),
    (
        "copies of headers and footers in sections have reached their limit",
        None,
    ),
    ("a table nested more than", None),
    ("a table without rows", None),
    ("a row without cells", None),
    ("a symbol beyond u+ffff", None),
    ("a reference to ", None),
    ("a font name of more than", None),
    ("a language tag of more than", None),
    ("table rows of more than 63 cells", None),
    ("table columns wider than", None),
    ("cells past the columns of their table's grid", None),
    ("a line or table of a footnote taller than the body", None),
    ("headers and footers are left out past", None),
    ("embedded font program", None),
    ("lines thicker than", None),
    ("pages narrower or shorter than", None),
    ("has no height; the lines are single", None),
    ("a cell that does not end with a paragraph", None),
    // Not required: fonts the layout cannot use or embed, which rows 3 and 11 look at.
    ("the layout tables of", None),
    ("is not drawn: ", None),
    ("is not installed: its symbols are drawn", None),
    // Not required: fields shown with their stored result, as phase 1 lays them out.
    ("fields other than page, numpages and sectionpages", None),
];

/// The class of a report entry.
pub fn classify(kind: &str, message: &str) -> Class {
    if kind != "feature-unsupported" {
        return Class::NotRequired;
    }
    let message = message.to_lowercase();
    match MESSAGES.iter().find(|(words, _)| message.contains(words)) {
        Some((_, Some(feature))) => Class::Required(feature),
        Some((_, None)) => Class::NotRequired,
        None => Class::Unclassified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The messages the layout and the translation into its IR write at the time of
    /// writing, each with its class.
    #[test]
    fn the_messages_of_today_are_classed() {
        let cases = [
            (
                "paragraph borders are not drawn yet",
                Class::Required("paragraph borders"),
            ),
            (
                "endnotes are not laid out yet: their texts are left out",
                Class::Required("notes"),
            ),
            (
                "footnotes referenced in table cells are not laid out yet: their texts are \
                 left out",
                Class::Required("notes"),
            ),
            (
                "cropped, turned or mirrored pictures are drawn whole and upright",
                Class::Required("pictures"),
            ),
            (
                "a picture is drawn as a grey box: the PNG data is damaged",
                Class::Required("pictures"),
            ),
            (
                "rows that a vertically merged cell ties together are not split across pages",
                Class::Required("tables"),
            ),
            (
                "a PAGE field in table header rows repeated on later pages shows the number \
                 of the page they were first laid out for",
                Class::Required("tables"),
            ),
            (
                "text does not wrap around floating objects yet: it runs over or under them",
                Class::NotRequired,
            ),
            (
                "pictures in emf format are drawn as grey boxes",
                Class::NotRequired,
            ),
            (
                "floating tables are laid out in the text where they are anchored",
                Class::NotRequired,
            ),
            (
                "a VML drawing that shows no picture or text box (a shape or group) is left \
                 out (w:pict)",
                Class::NotRequired,
            ),
            (
                "a reference to footnote 2, which is the separator, is left out",
                Class::NotRequired,
            ),
            (
                "a VML picture's text (v:textbox) is left out, here and wherever else the \
                 document has some",
                Class::Required("text boxes"),
            ),
            (
                "a VML picture's shadow (v:shadow) is not drawn, here and wherever else the \
                 document has one",
                Class::NotRequired,
            ),
            (
                "a chain of linked text boxes (wps:linkedTxbx, or mso-next-textbox in VML) is \
                 drawn with all its text in its first box",
                Class::NotRequired,
            ),
            (
                "vertical text (w:textDirection) is laid out horizontally, here and in every \
                 other section that has it",
                Class::NotRequired,
            ),
            (
                "a table row deleted or moved away as a tracked change (w:del) is kept",
                Class::NotRequired,
            ),
            (
                "table header rows are not repeated past 4000 shapes and glyphs that table \
                 rows draw again on later pages",
                Class::NotRequired,
            ),
            (
                "table header rows are not repeated on a page where they leave no room for \
                 the row after them",
                Class::Required("tables"),
            ),
            (
                "list numbers in words and ordinals in languages other than English and \
                 German are written in English",
                Class::Required("lists"),
            ),
            (
                "numbers in words and ordinals in languages other than English and German \
                 are written in English",
                Class::Required("fields"),
            ),
            (
                "right-to-left text is laid out from left to right, its characters in the \
                 order they are stored",
                Class::NotRequired,
            ),
            // Markup the translation does not know: possibly text that is lost.
            ("a block-level w:altChunk is left out", Class::Unclassified),
            (
                "an inline foo element is left out (w:foo)",
                Class::Unclassified,
            ),
            ("12 more entries are not listed", Class::Unclassified),
        ];
        for (message, class) in cases {
            assert_eq!(classify("feature-unsupported", message), class, "{message}");
        }
    }

    #[test]
    fn only_unsupported_features_degrade_one() {
        for kind in [
            "font-substituted",
            "field-cached",
            "compat-ignored",
            "overflow",
        ] {
            assert_eq!(
                classify(kind, "paragraph borders are not drawn yet"),
                Class::NotRequired,
                "{kind}"
            );
        }
    }

    /// The words are compared in lower case, so each line must be written in lower case
    /// to match at all.
    #[test]
    fn the_words_are_in_lower_case() {
        for (words, _) in MESSAGES {
            assert_eq!(words, words.to_lowercase(), "{words}");
        }
    }
}
