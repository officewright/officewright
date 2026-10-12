use std::fmt;
use std::hash::{Hash, Hasher};

use crate::{Error, Result};

/// The name of a part, such as `/word/document.xml`.
///
/// Names are compared as ECMA-376 Part 2 requires: ASCII letters without regard to case,
/// and percent-encoded characters as the characters they encode.
///
/// The checks keep names unambiguous and inside the package: no empty, `.` or `..`
/// segments, no segment ending in a dot, no `\`, no encoded `/`, no control characters.
/// They do not make a name a valid file name on every system (Windows refuses `C:` or
/// `CON`, for example); code that writes parts to disk has to check that itself.
/// The Open Packaging Conventions are stricter still (IRI characters only), but Office
/// packages in the wild contain names such as `/[trash]/0000.dat`, which Office opens.
///
/// ```
/// use officewright_opc::PartName;
///
/// let name = PartName::new("/word/media/image%201.PNG")?;
/// assert_eq!(name, PartName::new("/Word/Media/image 1.png")?);
/// assert_eq!(name.extension(), Some("PNG"));
/// assert!(PartName::new("/word/../secret").is_err());
/// # Ok::<(), officewright_opc::Error>(())
/// ```
#[derive(Clone, Debug)]
pub struct PartName {
    name: String,
    /// The name decoded and in ASCII lower case, for comparisons.
    key: String,
}

impl PartName {
    /// Checks a part name.
    ///
    /// # Errors
    ///
    /// [`Error::Package`] for a name that breaks one of the rules above.
    pub fn new(name: &str) -> Result<Self> {
        let invalid = |why: &str| Error::Package {
            message: format!("invalid part name {name:?}: {why}"),
        };
        let Some(path) = name.strip_prefix('/') else {
            return Err(invalid("it does not start with '/'"));
        };
        let mut key = String::with_capacity(name.len());
        for segment in path.split('/') {
            if segment.is_empty() {
                return Err(invalid("it has an empty segment"));
            }
            let decoded = percent_decode(segment)
                .ok_or_else(|| invalid("it has a malformed percent-encoding"))?;
            if decoded.contains(['/', '\\']) {
                return Err(invalid("it has a '\\' or an encoded '/'"));
            }
            if decoded.chars().any(char::is_control) {
                return Err(invalid("it has a control character"));
            }
            if decoded.ends_with('.') {
                return Err(invalid("a segment ends with '.'"));
            }
            key.push('/');
            key.extend(decoded.chars().map(|c| c.to_ascii_lowercase()));
        }
        Ok(Self {
            name: name.to_owned(),
            key,
        })
    }

    /// The name as written in the package.
    pub fn as_str(&self) -> &str {
        &self.name
    }

    /// The extension of the last segment, without the dot.
    pub fn extension(&self) -> Option<&str> {
        let file = self.name.rsplit('/').next()?;
        file.rsplit_once('.').map(|(_, extension)| extension)
    }

    pub(crate) fn key(&self) -> &str {
        &self.key
    }

    /// The folder of the part: `/word` for `/word/document.xml`, empty for `/a.xml`.
    pub(crate) fn folder(&self) -> &str {
        self.name.rsplit_once('/').map_or("", |(folder, _)| folder)
    }

    /// For a relationships part such as `/word/_rels/document.xml.rels`, the part its
    /// relationships belong to: `Some(Some(/word/document.xml))`, or `Some(None)` for
    /// the package relationships `/_rels/.rels`. `None` for any other part.
    pub(crate) fn relationships_source(&self) -> Option<Option<PartName>> {
        let (folder, file) = self.name.rsplit_once('/')?;
        let (parent, rels) = folder.rsplit_once('/')?;
        if !rels.eq_ignore_ascii_case("_rels") || file.len() < ".rels".len() {
            return None;
        }
        let (stem, extension) = file.split_at_checked(file.len() - ".rels".len())?;
        if !extension.eq_ignore_ascii_case(".rels") {
            return None;
        }
        match (parent, stem) {
            ("", "") => Some(None),
            (_, "") => None,
            _ => PartName::new(&format!("{parent}/{stem}")).ok().map(Some),
        }
    }
}

impl PartialEq for PartName {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for PartName {}

impl Hash for PartName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key.hash(state);
    }
}

impl fmt::Display for PartName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

/// Decodes `%XX` escapes; `None` for a malformed escape or bytes that are not UTF-8.
pub(crate) fn percent_decode(segment: &str) -> Option<String> {
    if !segment.contains('%') {
        return Some(segment.to_owned());
    }
    let mut bytes = Vec::with_capacity(segment.len());
    let mut rest = segment.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        if byte == b'%' {
            let hex = std::str::from_utf8(tail.get(..2)?).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            rest = tail.get(2..)?;
        } else {
            bytes.push(byte);
            rest = tail;
        }
    }
    String::from_utf8(bytes).ok()
}

/// Resolves an internal relationship target against the folder of its source part
/// (RFC 3986, section 5.2). `None` when the target cannot name a part: it is empty, has
/// a scheme, a query or a fragment, climbs out of the package, or is so long that joined
/// to the folder it is over `max_length` bytes, the longest part name a package may have,
/// which keeps the work of resolving it bounded.
pub(crate) fn resolve_target(folder: &str, target: &str, max_length: usize) -> Option<PartName> {
    if target.is_empty() || target.contains(['#', '?']) {
        return None;
    }
    let joined_length = if target.starts_with('/') {
        target.len() - 1
    } else {
        folder.len() + target.len()
    };
    if joined_length > max_length {
        return None;
    }
    if target
        .split_once(':')
        .is_some_and(|(scheme, _)| !scheme.contains('/'))
    {
        return None;
    }
    let joined = if target.starts_with('/') {
        target.to_owned()
    } else {
        format!("{folder}/{target}")
    };
    let mut segments = Vec::new();
    for segment in joined.split('/').skip(1) {
        match segment {
            "." => {}
            ".." => {
                segments.pop()?;
            }
            _ => segments.push(segment),
        }
    }
    PartName::new(&format!("/{}", segments.join("/"))).ok()
}
