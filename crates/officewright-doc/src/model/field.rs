//! Field instructions (ECMA-376 Part 1, §17.16.1): a field's name, then its arguments
//! and switches, as a simple field (`w:fldSimple`) writes them in `w:instr` and a complex
//! field in the `w:instrText` of its runs; and the instruction of a `HYPERLINK` field
//! (§17.16.5.25) read by its switches.

use std::fmt;

/// A field instruction as its syntax (ECMA-376 Part 1, §17.16.1) divides it: the
/// field's name, then its arguments and switches in the order written.
///
/// Reading never fails: whatever the text, it is divided as far as the syntax goes. An
/// argument is a run of characters up to white space, or text in quotes, in which `\"`
/// stands for a quote and `\\` for a backslash; a backslash before any other character in
/// quotes is kept with it. A switch is a backslash and the character after it, such as
/// `\l` or the formatting switch `\*`. Word requires white space after the field's name
/// (MS-OI29500 §17.16.1(a)), which the name therefore ends at.
///
/// Which switches take an argument depends on the field: [`switch`](Self::switch) gives
/// the argument written after a switch, where there is one. Written back with `Display`,
/// an instruction reads again as the same instruction.
///
/// ```
/// use officewright_doc::{FieldInstruction, FieldToken};
///
/// let instruction = FieldInstruction::parse(r#"HYPERLINK "https://example.com/a b" \o "Say \"hi\"""#);
/// assert_eq!(instruction.name(), "HYPERLINK");
/// assert_eq!(
///     instruction.tokens(),
///     [
///         FieldToken::Argument("https://example.com/a b".into()),
///         FieldToken::Switch('o'),
///         FieldToken::Argument(r#"Say "hi""#.into()),
///     ]
/// );
/// assert_eq!(instruction.switch('o'), Some(Some(r#"Say "hi""#)));
/// assert_eq!(FieldInstruction::parse(&instruction.to_string()), instruction);
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FieldInstruction {
    name: String,
    tokens: Vec<FieldToken>,
}

/// What follows a field's name in its instruction.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FieldToken {
    /// An argument: a word, or text in quotes without them, its escapes read.
    Argument(String),
    /// A switch: the character after its backslash, such as `l` for `\l` or `*` for `\*`.
    Switch(char),
}

impl FieldInstruction {
    /// Divides `instruction` into the field's name, arguments and switches.
    pub fn parse(instruction: &str) -> Self {
        let mut chars = instruction.chars().peekable();
        let mut name = String::new();
        let mut tokens = Vec::new();
        let mut first = true;
        loop {
            while chars.next_if(|c| c.is_whitespace()).is_some() {}
            let Some(&c) = chars.peek() else {
                break;
            };
            if c == '"' {
                chars.next();
                let mut text = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.peek() {
                            Some(&escaped @ ('"' | '\\')) => {
                                chars.next();
                                text.push(escaped);
                            }
                            _ => text.push('\\'),
                        },
                        c => text.push(c),
                    }
                }
                tokens.push(FieldToken::Argument(text));
            } else if c == '\\' {
                chars.next();
                // A backslash at the end has no switch character: it is an argument.
                match chars.next() {
                    Some(switch) if !switch.is_whitespace() => {
                        tokens.push(FieldToken::Switch(switch));
                    }
                    _ => tokens.push(FieldToken::Argument("\\".into())),
                }
            } else {
                let mut word = String::new();
                while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
                    word.push(c);
                }
                if first {
                    name = word;
                } else {
                    tokens.push(FieldToken::Argument(word));
                }
            }
            first = false;
        }
        Self { name, tokens }
    }

    /// The field's name as written, such as `HYPERLINK` or `PAGE`; Word reads it in any
    /// case. Empty where the instruction does not start with one.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The arguments and switches after the name, in order.
    pub fn tokens(&self) -> &[FieldToken] {
        &self.tokens
    }

    /// Whether the switch `switch` is written, and with which argument: `None` where it is
    /// not, `Some(None)` where no argument follows it, and `Some(Some(argument))` for the
    /// argument right after it. Of a switch written more than once, the first counts.
    pub fn switch(&self, switch: char) -> Option<Option<&str>> {
        let at = self
            .tokens
            .iter()
            .position(|token| *token == FieldToken::Switch(switch))?;
        Some(match self.tokens.get(at + 1) {
            Some(FieldToken::Argument(argument)) => Some(argument.as_str()),
            _ => None,
        })
    }

    /// The instruction of a `HYPERLINK` field read by its switches, or `None` for another
    /// field.
    pub fn hyperlink(&self) -> Option<HyperlinkField> {
        self.name
            .eq_ignore_ascii_case("HYPERLINK")
            .then(|| HyperlinkField::read(&self.tokens))
    }
}

impl fmt::Display for FieldInstruction {
    /// The instruction written back: the name, then each argument and switch after a
    /// space, an argument in quotes, with its quotes and backslashes escaped, unless it is
    /// a word without quotes and backslashes, which reads back as itself.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut space = if self.name.is_empty() {
            ""
        } else {
            f.write_str(&self.name)?;
            " "
        };
        for token in &self.tokens {
            // The first argument where there is no name, which as a word would read back
            // as the name.
            let leads = space.is_empty();
            f.write_str(space)?;
            space = " ";
            match token {
                FieldToken::Switch(switch) => write!(f, "\\{switch}")?,
                FieldToken::Argument(argument) => {
                    let word = !leads
                        && !argument.is_empty()
                        && !argument
                            .contains(|c: char| c.is_whitespace() || matches!(c, '"' | '\\'));
                    if word {
                        f.write_str(argument)?;
                    } else {
                        f.write_str("\"")?;
                        for c in argument.chars() {
                            if matches!(c, '"' | '\\') {
                                f.write_str("\\")?;
                            }
                            write!(f, "{c}")?;
                        }
                        f.write_str("\"")?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// The instruction of a `HYPERLINK` field (ECMA-376 Part 1, §17.16.5.25), read by its
/// switches: what the hyperlink leads to, and how it opens.
///
/// The switches that take an argument are `\l`, `\o`, `\t` and Word's `\s`, the others
/// none; the first argument that no such switch takes is the target. Word reads `\s` as
/// the `w:docLocation` of a hyperlink, and `\h` as a link that goes into no list of those
/// followed (MS-OI29500 §17.16.5.25(a), (b)), switches the standard does not list.
///
/// ```
/// use officewright_doc::FieldInstruction;
///
/// let link = FieldInstruction::parse(r#"HYPERLINK \l "Summary" \o "Go there""#)
///     .hyperlink()
///     .unwrap();
/// assert_eq!(link.target(), None);
/// assert_eq!(link.bookmark(), Some("Summary"));
/// assert_eq!(link.tooltip(), Some("Go there"));
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HyperlinkField {
    target: Option<String>,
    bookmark: Option<String>,
    doc_location: Option<String>,
    target_frame: Option<String>,
    tooltip: Option<String>,
    image_map: bool,
    new_window: bool,
    no_history: bool,
}

impl HyperlinkField {
    fn read(tokens: &[FieldToken]) -> Self {
        let mut link = Self::default();
        let mut tokens = tokens.iter();
        while let Some(token) = tokens.next() {
            match token {
                FieldToken::Argument(argument) => {
                    if link.target.is_none() {
                        link.target = Some(argument.clone());
                    }
                }
                FieldToken::Switch(switch) => {
                    let slot = match switch {
                        'l' => &mut link.bookmark,
                        's' => &mut link.doc_location,
                        't' => &mut link.target_frame,
                        'o' => &mut link.tooltip,
                        'm' => {
                            link.image_map = true;
                            continue;
                        }
                        'n' => {
                            link.new_window = true;
                            continue;
                        }
                        'h' => {
                            link.no_history = true;
                            continue;
                        }
                        // A formatting switch, such as `\*`, takes its argument too.
                        _ => {
                            let mut rest = tokens.clone();
                            if matches!(rest.next(), Some(FieldToken::Argument(_))) {
                                tokens = rest;
                            }
                            continue;
                        }
                    };
                    let mut rest = tokens.clone();
                    if let Some(FieldToken::Argument(argument)) = rest.next() {
                        tokens = rest;
                        if slot.is_none() {
                            *slot = Some(argument.clone());
                        }
                    }
                }
            }
        }
        link
    }

    /// What the hyperlink leads to outside the document, such as a web address or a
    /// file: its first argument.
    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }

    /// The bookmark it leads to (`\l`).
    pub fn bookmark(&self) -> Option<&str> {
        self.bookmark.as_deref()
    }

    /// A place in the target that is no bookmark (`\s`), which Word reads as a
    /// hyperlink's `w:docLocation`.
    pub fn doc_location(&self) -> Option<&str> {
        self.doc_location.as_deref()
    }

    /// The frame or window the target opens in (`\t`).
    pub fn target_frame(&self) -> Option<&str> {
        self.target_frame.as_deref()
    }

    /// The text shown over the hyperlink (`\o`).
    pub fn tooltip(&self) -> Option<&str> {
        self.tooltip.as_deref()
    }

    /// Whether the coordinates of a server-side image map are appended to the target
    /// (`\m`).
    pub fn image_map(&self) -> bool {
        self.image_map
    }

    /// Whether the target opens in a new window (`\n`).
    pub fn new_window(&self) -> bool {
        self.new_window
    }

    /// Whether following the hyperlink adds its target to no list of those followed
    /// (`\h`).
    pub fn no_history(&self) -> bool {
        self.no_history
    }
}
