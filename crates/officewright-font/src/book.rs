use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufReader, Cursor};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;
use std::{error, fmt};

use crate::coverage::Coverage;
use crate::error::{Error, Result};
use crate::face::{EmbeddingPermissions, FaceProperties, FontNames, OutlineFormat};
use crate::font::FontFace;
use crate::index::{self, CoverageLimits, IndexError, IndexedFace};
use crate::system::system_font_dirs;

/// Where a face in a [`FontBook`] came from. When several faces fit a request equally
/// well, document fonts win over user fonts, and user fonts over system fonts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum FontOrigin {
    /// Embedded in the document being converted.
    Document,
    /// Added by the caller from a directory, file or memory.
    User,
    /// Installed on the system.
    System,
}

/// Identifies a face in the [`FontBook`] that returned it. Used with another book, it
/// names a different face or none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FaceId(usize);

/// A face in a [`FontBook`]: where it is and what the font index read from it.
#[derive(Clone, Debug)]
pub struct FaceInfo {
    source: Source,
    index: u32,
    origin: FontOrigin,
    properties: FaceProperties,
    keys: NameKeys,
    coverage: CoverageCell,
}

/// What a face maps: its place among the faces added from one file or buffer, whose
/// coverage is read together on the first query, kept, and shared by the clones of the
/// book.
#[derive(Clone)]
struct CoverageCell {
    file: Arc<FileCoverage>,
    slot: usize,
}

/// The faces added from one file or buffer, and what each maps once it is read.
struct FileCoverage {
    faces: Vec<u32>,
    coverages: OnceLock<Vec<Arc<Coverage>>>,
}

/// What a face covers when it cannot be read.
static NOTHING: Coverage = Coverage::NOTHING;

impl fmt::Debug for CoverageCell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let coverage = self.file.coverages.get().and_then(|all| all.get(self.slot));
        match coverage {
            Some(coverage) => write!(f, "{} ranges", coverage.range_count()),
            None => f.write_str("not read"),
        }
    }
}

#[derive(Clone)]
enum Source {
    File(PathBuf),
    Memory(Arc<[u8]>),
}

impl fmt::Debug for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(path) => f.debug_tuple("File").field(path).finish(),
            Self::Memory(data) => write!(f, "Memory({} bytes)", data.len()),
        }
    }
}

/// Normalized names for matching a requested family.
#[derive(Clone, Debug, Default)]
pub(crate) struct NameKeys {
    pub(crate) families: Vec<String>,
    pub(crate) typographic_families: Vec<String>,
    pub(crate) full_names: Vec<String>,
    pub(crate) postscript_name: Option<String>,
    pub(crate) instances: Vec<InstanceKey>,
}

/// A named instance of a variable font, for matching the names Word gives instances:
/// the family followed by the instance's subfamily words other than Regular, Bold and
/// Italic, such as "Bahnschrift SemiBold" or "Segoe UI Variable Display".
#[derive(Clone, Debug)]
pub(crate) struct InstanceKey {
    /// The instance's subfamily name as the font has it.
    pub(crate) name: String,
    /// The subfamily words that are part of the family name, normalized and sorted.
    pub(crate) words: Vec<String>,
    pub(crate) weight: u16,
    pub(crate) italic: bool,
}

/// How a family name is compared: Windows compares font names without regard to case.
pub(crate) fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

impl NameKeys {
    /// Whether the face answers to a request of the family with the normalized name `key`
    /// by its typographic family, full or PostScript name, which
    /// [`FontResolver`](crate::FontResolver) takes after the family names.
    pub(crate) fn has_other_name(&self, key: &str) -> bool {
        self.typographic_families.iter().any(|name| name == key)
            || self.full_names.iter().any(|name| name == key)
            || self.postscript_name.as_deref() == Some(key)
    }

    /// The named instance the normalized name `key` names: one of the face's family names
    /// followed by the instance's words other than Regular, Bold and Italic, in any order.
    pub(crate) fn instance(&self, key: &str) -> Option<&InstanceKey> {
        let families = self.families.iter().chain(&self.typographic_families);
        families.into_iter().find_map(|family| {
            let rest = key.strip_prefix(family.as_str())?.strip_prefix(' ')?;
            let mut words: Vec<&str> = rest.split_whitespace().collect();
            words.sort_unstable();
            self.instances
                .iter()
                .find(|instance| instance.words == words)
        })
    }

    fn new(face: &IndexedFace) -> Self {
        let names = &face.properties.names;
        let instances = face
            .instances
            .iter()
            .filter_map(|instance| {
                let mut words: Vec<String> = name_key(&instance.name)
                    .split_whitespace()
                    .filter(|word| !matches!(*word, "regular" | "bold" | "italic"))
                    .map(str::to_owned)
                    .collect();
                words.sort();
                // An instance with only style words is the family itself.
                (!words.is_empty()).then(|| InstanceKey {
                    name: instance.name.clone(),
                    words,
                    weight: instance.weight,
                    italic: instance.italic,
                })
            })
            .collect();
        Self {
            families: names.families().map(name_key).collect(),
            typographic_families: names.typographic_families().map(name_key).collect(),
            full_names: names.full_names().map(name_key).collect(),
            postscript_name: names.postscript_name().map(name_key),
            instances,
        }
    }
}

impl FaceInfo {
    /// Where the face came from.
    pub fn origin(&self) -> FontOrigin {
        self.origin
    }

    /// The file the face is in, `None` for a font added from memory.
    pub fn path(&self) -> Option<&Path> {
        match &self.source {
            Source::File(path) => Some(path),
            Source::Memory(_) => None,
        }
    }

    /// The index of the face in its file.
    pub fn index(&self) -> u32 {
        self.index
    }

    /// The names of the face.
    pub fn names(&self) -> &FontNames {
        &self.properties.names
    }

    /// The weight class, 1 to 1000; 400 is regular, 700 bold.
    pub fn weight(&self) -> u16 {
        self.properties.weight
    }

    /// Whether the face is italic or oblique.
    pub fn is_italic(&self) -> bool {
        self.properties.italic
    }

    /// Whether the face is its family's bold one: its style bits say so, and its subfamily
    /// name, if it has one, says Bold. A heavy family of its own, such as Arial Black, is
    /// not, whatever its weight; nor is one whose bits say bold but whose subfamily is
    /// "Regular", as Yu Gothic UI Semibold's do.
    pub(crate) fn is_bold(&self) -> bool {
        self.properties.bold
            && self
                .properties
                .names
                .subfamily()
                .is_none_or(|subfamily| subfamily.to_lowercase().contains("bold"))
    }

    /// What the font license allows when the face is embedded in a document.
    pub fn embedding(&self) -> EmbeddingPermissions {
        self.properties.embedding
    }

    /// How the glyph outlines are stored, `None` for a face without outlines.
    pub fn outline_format(&self) -> Option<OutlineFormat> {
        self.properties.outlines
    }

    /// Whether the face is a variable font.
    pub fn is_variable(&self) -> bool {
        self.properties.variable
    }

    pub(crate) fn keys(&self) -> &NameKeys {
        &self.keys
    }

    /// The characters the face maps. The first call reads the `cmap` of every face
    /// added from the same file or buffer; a face that cannot be read any more, or whose
    /// `cmap` is damaged, maps nothing, and is not read again.
    fn coverage(&self) -> &Coverage {
        let file = &self.coverage.file;
        let all = file.coverages.get_or_init(|| match &self.source {
            Source::File(path) => match File::open(path) {
                Ok(open) => index::read_coverages(
                    &mut BufReader::new(open),
                    &file.faces,
                    CoverageLimits::FILE,
                ),
                Err(_) => Vec::new(),
            },
            Source::Memory(data) => {
                index::read_coverages(&mut Cursor::new(&**data), &file.faces, CoverageLimits::FILE)
            }
        });
        all.get(self.coverage.slot)
            .map_or(&NOTHING, |coverage| coverage)
    }
}

/// A directory, font file or face of a font file that a [`FontBook`] could not read or
/// index.
#[derive(Clone, Debug)]
pub struct SkippedFont {
    path: Option<PathBuf>,
    /// Shared, so that a [`FontBook`] can be cloned: an I/O error cannot be.
    error: Arc<Error>,
    /// The key in [`FontBook::failed`] this entry is listed under, so a later scan can
    /// replace or drop it; `None` for entries that stay, such as a damaged face of a
    /// collection that was added.
    key: Option<PathBuf>,
}

impl SkippedFont {
    /// The file or directory, `None` for a font added from memory.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Why it was skipped.
    pub fn error(&self) -> &Error {
        &self.error
    }
}

impl fmt::Display for SkippedFont {
    /// The file, the error and every reason behind it, such as `fonts/a.ttc: font face
    /// 64: the table directory is damaged: faces 64 to 1023 were not read: …`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.path, &*self.error) {
            // The error already names the file.
            (_, Error::Read { .. }) => {}
            (Some(path), _) => write!(f, "{}: ", path.display())?,
            (None, _) => f.write_str("font data: ")?,
        }
        self.error.fmt(f)?;
        let mut source = error::Error::source(&*self.error);
        while let Some(cause) = source {
            write!(f, ": {cause}")?;
            source = cause.source();
        }
        Ok(())
    }
}

/// The fonts available for a document: fonts it embeds, fonts the caller adds and
/// fonts installed on the system.
///
/// Adding a font reads only its names and style; [`load`](Self::load) reads the face
/// itself. Files that are not fonts or are damaged are skipped and listed in
/// [`skipped`](Self::skipped). A `FontBook` is `Send` and `Sync`; cloning it copies the
/// index, not the font data.
///
/// A book can scan the same directories again, for example after fonts were installed:
/// [`add_dir`](Self::add_dir) and [`add_system_fonts`](Self::add_system_fonts) add the
/// fonts that appeared since. A file that was added is not read again, even if it
/// changed. A file that was skipped is read again once its size or modification time
/// changes, so a font that was still being copied during one scan is found by the next.
/// Links, font files that could not be resolved and directories that could not be read
/// are tried on every scan. Each skipped path is listed once, with the error of its
/// latest attempt, and leaves the list when it is read. A path that is removed stays
/// listed; a process that wants a fresh list starts a new book.
///
/// ```
/// use officewright_font::{FontBook, FontRequest, FontResolver};
///
/// let mut book = FontBook::new();
/// book.add_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data"))?;
/// let mut resolver = FontResolver::new(&book);
/// let resolution = resolver.resolve(&FontRequest::new("Officewright Test"));
/// let font = book.load(resolution.face().expect("the font is in the book"))?;
/// assert_eq!(font.names().family(), Some("Officewright Test"));
/// # Ok::<(), officewright_font::Error>(())
/// ```
#[derive(Clone, Debug, Default)]
pub struct FontBook {
    faces: Vec<FaceInfo>,
    skipped: Vec<SkippedFont>,
    /// Canonical paths of the files already added, so a font reachable through
    /// several directories or links is indexed once.
    files: HashSet<PathBuf>,
    /// The paths a directory scan could not add, each listed once in `skipped`: the
    /// canonical paths of files that could not be indexed, and the canonical directory
    /// and name of font files and directories that could not be resolved or read.
    failed: HashMap<PathBuf, Failure>,
    /// The canonical directory behind each location in `failed` of a directory that could
    /// not be read, so that reading that directory by any path drops them all.
    unreadable_dirs: HashMap<PathBuf, PathBuf>,
}

/// How a path that a directory scan could not add looked, so the next scan tries it
/// again only when that changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failure {
    /// The path could not be resolved or read, such as a link to a removed file or a
    /// directory without permission. Trying again reads no font data.
    Unreachable,
    /// The file could not be indexed when it had this size and modification time.
    File {
        len: u64,
        modified: Option<SystemTime>,
    },
}

impl Failure {
    fn file(metadata: &fs::Metadata) -> Self {
        Self::File {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        }
    }
}

/// How deep [`FontBook::add_dir`] descends; font directories are rarely more than a
/// few levels deep.
const MAX_DEPTH: usize = 16;

impl FontBook {
    /// An empty book.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the fonts in [`system_font_dirs`] and their subdirectories, and returns
    /// the number of faces added. Directories that do not exist are left out.
    ///
    /// Called again, it adds the fonts installed since and tries the skipped files
    /// again as the [`FontBook`] documentation describes.
    pub fn add_system_fonts(&mut self) -> usize {
        let mut added = 0;
        for dir in system_font_dirs() {
            if dir.is_dir() {
                added += self.scan_dir(&dir, FontOrigin::System);
            }
        }
        added
    }

    /// Adds the fonts in a directory and its subdirectories, and returns the number of
    /// faces added.
    ///
    /// Called again, it adds the fonts that appeared since and tries the skipped files
    /// again as the [`FontBook`] documentation describes.
    ///
    /// # Errors
    ///
    /// [`Error::Read`] if the directory cannot be read. Files in it that cannot be
    /// read or are not fonts are skipped.
    pub fn add_dir(&mut self, dir: impl AsRef<Path>) -> Result<usize> {
        let dir = dir.as_ref();
        fs::read_dir(dir).map_err(|source| Error::Read {
            path: dir.to_owned(),
            source,
        })?;
        Ok(self.scan_dir(dir, FontOrigin::User))
    }

    /// Adds the faces of a font file, and returns the number of faces added.
    ///
    /// The file is read even if a directory scan skipped it before. If it cannot be
    /// added, the error is returned rather than listed in [`skipped`](Self::skipped).
    ///
    /// # Errors
    ///
    /// [`Error::Read`] if the file cannot be read or is not a regular file, and the errors of
    /// [`FontFace::from_bytes`] if it is not a font or every face in it is damaged. Damaged
    /// faces of a collection whose other faces are fine are skipped.
    pub fn add_file(&mut self, path: impl AsRef<Path>) -> Result<usize> {
        self.add_file_with(path.as_ref(), FontOrigin::User)
    }

    /// Adds the faces of a font in memory, such as one embedded in a document, and
    /// returns the number of faces added.
    ///
    /// # Errors
    ///
    /// The errors of [`FontFace::from_bytes`] if the data is not a font or every face in
    /// it is damaged. Damaged faces of a collection whose other faces are fine are
    /// skipped.
    pub fn add_bytes(&mut self, data: impl Into<Arc<[u8]>>, origin: FontOrigin) -> Result<usize> {
        let data = data.into();
        let faces = match index::read_faces(&mut Cursor::new(&*data)) {
            Ok(faces) => faces,
            Err(IndexError::Font(err)) => return Err(err),
            // reason: reading from memory has no I/O to fail; report it as data error.
            Err(IndexError::Io(err)) => {
                return Err(Error::NotAFont {
                    source: crate::error::Cause::new(err.to_string()),
                });
            }
        };
        self.add_faces(&Source::Memory(data), origin, faces)
    }

    /// [`add_bytes`](Self::add_bytes), and the faces added also answer to `family`, as one
    /// of their own family names: a [`FontResolver`](crate::FontResolver) finds them for a
    /// request of that family as it finds a face by its family name. Their own names stay,
    /// and so does the style each face's own tables give, which decides the requests of
    /// the family it serves.
    ///
    /// For a font a document embeds under a name it does not carry itself: Word renames
    /// the program of a font it embeds as a subset (`w:subsetted`) to `___WRD_EMBED_SUB_`
    /// and a number, while the document's text keeps naming the font, such as "IBM Plex
    /// Serif Light". Word renames the family and full names only, so a face may still
    /// answer to the document's name by its PostScript or typographic name. Of the faces
    /// added, as a collection has several, those that answer to the name by another name
    /// of their own get it, if any; none gets it where one has it as a family name or
    /// names a named instance by it, since that face serves it as it is. A `family` that
    /// is empty or only white space, or longer than the 512 bytes the book keeps of a
    /// name, adds no name.
    ///
    /// ```
    /// use officewright_font::{FontBook, FontOrigin, FontRequest, FontResolver, ResolutionKind};
    ///
    /// let data = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/minimal.ttf"))?;
    /// let mut book = FontBook::new();
    /// book.add_bytes_as(data, FontOrigin::Document, "IBM Plex Serif Light")?;
    /// let mut resolver = FontResolver::new(&book);
    /// let resolution = resolver.resolve(&FontRequest::new("IBM Plex Serif Light"));
    /// assert_eq!(resolution.kind(), ResolutionKind::Exact);
    /// // The face keeps its own name too.
    /// let resolution = resolver.resolve(&FontRequest::new("Officewright Test"));
    /// assert_eq!(resolution.kind(), ResolutionKind::Exact);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Those of [`add_bytes`](Self::add_bytes).
    pub fn add_bytes_as(
        &mut self,
        data: impl Into<Arc<[u8]>>,
        origin: FontOrigin,
        family: &str,
    ) -> Result<usize> {
        let first = self.faces.len();
        let added = self.add_bytes(data, origin)?;
        self.add_family(first, family);
        Ok(added)
    }

    /// [`add_file`](Self::add_file), and the faces added also answer to `family`, as
    /// [`add_bytes_as`](Self::add_bytes_as) has them. A file the book has already read adds
    /// no faces, and so no name.
    ///
    /// # Errors
    ///
    /// Those of [`add_file`](Self::add_file).
    pub fn add_file_as(&mut self, path: impl AsRef<Path>, family: &str) -> Result<usize> {
        let first = self.faces.len();
        let added = self.add_file(path)?;
        self.add_family(first, family);
        Ok(added)
    }

    /// Gives the faces from `first` on the family name `family`: none where one of them has
    /// it as a family name or names a named instance by it, since the resolver finds that
    /// face by it as it is; else those that answer to it by another name of their own
    /// ([`NameKeys::has_other_name`]: a typographic family, full or PostScript name, taken
    /// together, though the resolver tries them one after another), as the regular face of
    /// a subset Word renamed often does by its PostScript name; else all. Nor where the name, without the white space
    /// around it, is empty or too long to keep.
    fn add_family(&mut self, first: usize, family: &str) {
        if family.trim().len() > crate::face::MAX_NAME_BYTES {
            return;
        }
        let key = name_key(family);
        if key.is_empty() {
            return;
        }
        let faces = self.faces.get_mut(first..).unwrap_or_default();
        let has_family = |face: &FaceInfo| {
            face.keys.families.contains(&key) || face.keys.instance(&key).is_some()
        };
        if faces.iter().any(has_family) {
            return;
        }
        let some_answer = faces.iter().any(|face| face.keys.has_other_name(&key));
        for face in faces {
            if !some_answer || face.keys.has_other_name(&key) {
                face.keys.families.push(key.clone());
            }
        }
    }

    /// The faces whose character map has a glyph for `ch`, in the order of
    /// [`faces`](Self::faces).
    ///
    /// A face covers the characters that the subtable [`FontFace::char_map`] uses maps to
    /// a glyph other than 0 and inside the font, exactly the characters
    /// [`CharMap::map`](crate::CharMap::map) maps, with three exceptions. A symbol font
    /// covers the characters its subtable lists, normally U+F020 to U+F0FF; the U+0020 to
    /// U+00FF that [`FontFace::glyph_id`] also finds, as Windows does, do not count, so a
    /// symbol font never stands in for Latin text. A subtable whose segments or groups are
    /// out of order or overlap, so that a lookup cannot be trusted, covers nothing; a
    /// repeated closing segment of a format 4 subtable does not count as overlap. And a
    /// format 4 subtable never covers U+FFFF, a noncharacter.
    ///
    /// Adding fonts does not read their character maps. The first query reads, for every
    /// face added from the same file or buffer, its table directory, its `cmap` table and
    /// the start of its `maxp` table, and keeps what each face maps as ranges of
    /// characters, so later queries cost a lookup per face; the clones of the book share
    /// it. Faces that share their tables, as those of a collection usually do, are read
    /// once. The work grows with the size of the tables, not with the number of
    /// characters they map, and is bounded per file like the index: the bytes it reads,
    /// about four million segments, groups and glyph-array entries, and 65,536 ranges,
    /// for all its faces together, which real fonts stay far below. A face past those
    /// bounds, a face whose file can no longer be read, and a face whose `cmap` is damaged
    /// or missing cover nothing, and are not read again.
    ///
    /// ```
    /// use officewright_font::FontBook;
    ///
    /// let mut book = FontBook::new();
    /// book.add_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data"))?;
    /// assert_eq!(book.faces_covering('A').len(), 1);
    /// assert!(book.faces_covering('\u{4E00}').is_empty());
    /// # Ok::<(), officewright_font::Error>(())
    /// ```
    pub fn faces_covering(&self, ch: char) -> Vec<FaceId> {
        self.faces()
            .filter(|(_, face)| face.coverage().contains(ch))
            .map(|(id, _)| id)
            .collect()
    }

    /// The faces in the book, in the order they were added.
    pub fn faces(&self) -> impl ExactSizeIterator<Item = (FaceId, &FaceInfo)> {
        self.faces
            .iter()
            .enumerate()
            .map(|(id, face)| (FaceId(id), face))
    }

    /// A face, `None` if the ID is not from this book.
    pub fn face(&self, id: FaceId) -> Option<&FaceInfo> {
        self.faces.get(id.0)
    }

    /// The number of faces.
    pub fn len(&self) -> usize {
        self.faces.len()
    }

    /// Whether the book has no faces.
    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    /// The directories, files and faces that could not be read or indexed, each path
    /// with the error of its latest attempt.
    pub fn skipped(&self) -> &[SkippedFont] {
        &self.skipped
    }

    /// Reads a face. A face added from a file reads the whole file again; keep the
    /// [`FontFace`] to use it more than once.
    ///
    /// # Errors
    ///
    /// [`Error::FaceIndexOutOfRange`] if the ID is not from this book, [`Error::Read`]
    /// if the file cannot be read any more, and the errors of [`FontFace::from_bytes`] if
    /// it changed and is no longer a readable font.
    pub fn load(&self, id: FaceId) -> Result<FontFace> {
        let face = self.face(id).ok_or(Error::FaceIndexOutOfRange {
            index: u32::try_from(id.0).unwrap_or(u32::MAX),
            count: u32::try_from(self.faces.len()).unwrap_or(u32::MAX),
        })?;
        let data: Arc<[u8]> = match &face.source {
            Source::File(path) => fs::read(path)
                .map_err(|source| Error::Read {
                    path: path.clone(),
                    source,
                })?
                .into(),
            Source::Memory(data) => Arc::clone(data),
        };
        FontFace::from_bytes(data, face.index)
    }

    fn add_file_with(&mut self, path: &Path, origin: FontOrigin) -> Result<usize> {
        let read_error = |source| Error::Read {
            path: path.to_owned(),
            source,
        };
        let canonical = fs::canonicalize(path).map_err(read_error)?;
        let metadata = fs::metadata(&canonical).map_err(read_error)?;
        let result = self.add_resolved_file(path, canonical, &metadata, origin);
        // A scan may have listed the path itself, such as a link that did not resolve.
        if result.is_ok() {
            if let Some(location) = location(path) {
                self.forget_failure(&location);
            }
        }
        result
    }

    /// Adds a file whose canonical path and metadata the caller has read.
    fn add_resolved_file(
        &mut self,
        path: &Path,
        canonical: PathBuf,
        metadata: &fs::Metadata,
        origin: FontOrigin,
    ) -> Result<usize> {
        if self.files.contains(&canonical) {
            return Ok(0);
        }
        // Opening a FIFO or device would block or read forever.
        let result = if metadata.is_file() {
            self.index_file(path, origin)
        } else {
            Err(Error::Read {
                path: path.to_owned(),
                source: io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"),
            })
        };
        if result.is_ok() {
            self.forget_failure(&canonical);
            self.files.insert(canonical);
        }
        result
    }

    /// Lists a path that a directory scan could not add, with the error of this attempt.
    /// A path that was already unreachable keeps its place in the list and the path it
    /// was first reached by, so reaching it through another alias of its directory does
    /// not change the entry; anything else listed for it before is replaced.
    fn list_failure(&mut self, key: PathBuf, failure: Failure, path: PathBuf, mut error: Error) {
        // A directory behind the path is remembered again by the caller if it is one.
        self.unreadable_dirs.remove(&key);
        if failure == Failure::Unreachable && self.failed.get(&key) == Some(&failure) {
            let listed = self
                .skipped
                .iter_mut()
                .find(|skipped| skipped.key.as_ref() == Some(&key));
            if let Some(listed) = listed {
                // The message names the path, so it keeps the listed one too.
                if let (Error::Read { path, .. }, Some(first)) = (&mut error, &listed.path) {
                    path.clone_from(first);
                }
                listed.error = Arc::new(error);
                return;
            }
        }
        self.forget_failure(&key);
        self.skipped.push(SkippedFont {
            path: Some(path),
            error: Arc::new(error),
            key: Some(key.clone()),
        });
        self.failed.insert(key, failure);
    }

    /// Drops what was listed for a path that has now been read.
    fn forget_failure(&mut self, key: &Path) {
        self.unreadable_dirs.remove(key);
        if self.failed.remove(key).is_some() {
            self.skipped
                .retain(|skipped| skipped.key.as_deref() != Some(key));
        }
    }

    fn index_file(&mut self, path: &Path, origin: FontOrigin) -> Result<usize> {
        let read_error = |source| Error::Read {
            path: path.to_owned(),
            source,
        };
        let mut file = BufReader::new(File::open(path).map_err(read_error)?);
        let faces = match index::read_faces(&mut file) {
            Ok(faces) => faces,
            Err(IndexError::Io(source)) => return Err(read_error(source)),
            Err(IndexError::Font(err)) => return Err(err),
        };
        self.add_faces(&Source::File(path.to_owned()), origin, faces)
    }

    fn add_faces(
        &mut self,
        source: &Source,
        origin: FontOrigin,
        faces: index::Faces,
    ) -> Result<usize> {
        let path = match source {
            Source::File(path) => Some(path.clone()),
            Source::Memory(_) => None,
        };
        let mut indexed = Vec::new();
        let mut errors = Vec::new();
        for (index, face) in faces {
            match face {
                Ok(face) => indexed.push((index, face)),
                Err(error) => errors.push(error),
            }
        }
        let added = indexed.len();
        let file = Arc::new(FileCoverage {
            faces: indexed.iter().map(|(index, _)| *index).collect(),
            coverages: OnceLock::new(),
        });
        for (slot, (index, face)) in indexed.into_iter().enumerate() {
            let keys = NameKeys::new(&face);
            self.faces.push(FaceInfo {
                source: source.clone(),
                index,
                origin,
                properties: face.properties,
                keys,
                coverage: CoverageCell {
                    file: Arc::clone(&file),
                    slot,
                },
            });
        }
        if added == 0 {
            if let Some(error) = errors.into_iter().next() {
                return Err(error);
            }
            return Ok(0);
        }
        self.skipped
            .extend(errors.into_iter().map(|error| SkippedFont {
                path: path.clone(),
                error: Arc::new(error),
                key: None,
            }));
        Ok(added)
    }

    /// Adds the fonts under `root`; unreadable files and directories are skipped.
    fn scan_dir(&mut self, root: &Path, origin: FontOrigin) -> usize {
        let mut added = 0;
        // Whether each directory reached in this scan could be read.
        let mut visited: HashMap<PathBuf, bool> = HashMap::new();
        // Each directory with its location, under which it is listed if it cannot be
        // read, as a font file that cannot be resolved is: the entry, not what it leads
        // to, so that replacing it with a link to another directory drops the entry. A
        // root without a name of its own, such as `.`, is listed under its canonical path.
        let mut stack = vec![(root.to_owned(), location(root), 0)];
        while let Some((dir, dir_location, depth)) = stack.pop() {
            // Following links can lead back to a directory already searched.
            let Ok(canonical_dir) = fs::canonicalize(&dir) else {
                continue;
            };
            let dir_location = dir_location.unwrap_or_else(|| canonical_dir.clone());
            // A directory reached again is not read again, but what is listed for this
            // entry follows where it leads now: nothing once that directory was read, and
            // that directory if it could not be read.
            if let Some(&read) = visited.get(&canonical_dir) {
                if read {
                    self.forget_failure(&dir_location);
                } else if self.unreadable_dirs.contains_key(&dir_location) {
                    self.unreadable_dirs.insert(dir_location, canonical_dir);
                }
                continue;
            }
            let entries = match fs::read_dir(&dir) {
                // Read now, under this path or another: what was listed for the entry,
                // and for every entry that leads to this directory (the directory itself
                // among them, reached by its own path), is out of date.
                Ok(entries) => {
                    visited.insert(canonical_dir.clone(), true);
                    self.forget_failure(&dir_location);
                    let behind: Vec<PathBuf> = self
                        .unreadable_dirs
                        .iter()
                        .filter(|(_, target)| **target == canonical_dir)
                        .map(|(listed, _)| listed.clone())
                        .collect();
                    // An entry leads where it leads now, not where it led when it was
                    // listed: one repointed since to another directory that cannot be read
                    // stays listed, behind that directory. One that leads here, to a
                    // directory that can be read, to something else or nowhere leaves the
                    // list, as a new book would not list it.
                    for listed in behind {
                        match fs::canonicalize(&listed) {
                            Ok(target)
                                if target != canonical_dir
                                    && target.is_dir()
                                    && fs::read_dir(&target).is_err() =>
                            {
                                self.unreadable_dirs.insert(listed, target);
                            }
                            _ => self.forget_failure(&listed),
                        }
                    }
                    entries
                }
                Err(source) => {
                    visited.insert(canonical_dir.clone(), false);
                    let error = Error::Read {
                        path: dir.clone(),
                        source,
                    };
                    self.list_failure(dir_location.clone(), Failure::Unreachable, dir, error);
                    self.unreadable_dirs.insert(dir_location, canonical_dir);
                    continue;
                }
            };
            // Sorted, so faces are added in the same order on every run: the files of a
            // directory, then its subdirectories.
            let mut paths: Vec<PathBuf> =
                entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
            paths.sort();
            let mut subdirs = Vec::new();
            for path in paths {
                // Where the entry is, however its directory was reached.
                let location = path
                    .file_name()
                    .map_or_else(|| path.clone(), |name| canonical_dir.join(name));
                // `metadata` follows links, so linked fonts and directories count.
                let metadata = fs::metadata(&path);
                if metadata.as_ref().is_ok_and(fs::Metadata::is_dir) {
                    if depth < MAX_DEPTH {
                        subdirs.push((path, Some(location), depth + 1));
                    }
                    continue;
                }
                if !is_font_file(&path) {
                    continue;
                }
                let resolved =
                    metadata.and_then(|metadata| Ok((fs::canonicalize(&path)?, metadata)));
                let (canonical, metadata) = match resolved {
                    Ok(resolved) => resolved,
                    // A font file that cannot be resolved, such as a link to a removed
                    // file, is listed under its location, once.
                    Err(source) => {
                        let error = Error::Read {
                            path: path.clone(),
                            source,
                        };
                        self.list_failure(location, Failure::Unreachable, path, error);
                        continue;
                    }
                };
                // What was listed under the location is out of date if the entry was
                // unresolvable and resolves now, or is now a link to another file. A
                // regular file is listed under its location, which is its canonical path,
                // and is compared below.
                if canonical != location
                    || self.failed.get(&location) == Some(&Failure::Unreachable)
                {
                    self.forget_failure(&location);
                }
                // Sockets, FIFOs and devices are not fonts, whatever their names.
                if !metadata.is_file() {
                    continue;
                }
                // A file that failed before is read again only once it has changed.
                let failure = Failure::file(&metadata);
                if self.failed.get(&canonical) == Some(&failure) {
                    continue;
                }
                match self.add_resolved_file(&path, canonical.clone(), &metadata, origin) {
                    Ok(count) => added += count,
                    Err(error) => self.list_failure(canonical, failure, path, error),
                }
            }
            stack.extend(subdirs.into_iter().rev());
        }
        added
    }
}

/// Where a directory entry is: its canonical directory and its name, however the
/// directory was reached. A scan lists a font file that cannot be resolved, and a
/// directory that cannot be read, under it.
fn location(path: &Path) -> Option<PathBuf> {
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    Some(fs::canonicalize(dir).ok()?.join(path.file_name()?))
}

/// Whether a file's extension is one of a TrueType or OpenType font or collection.
fn is_font_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ["ttf", "otf", "ttc", "otc"]
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, TestFont};
    use crate::{FontRequest, FontResolver, ResolutionKind};

    /// The family name of the face `resolver` resolves `request` to, and how.
    fn resolved(book: &FontBook, request: FontRequest) -> (Option<String>, ResolutionKind) {
        let resolution = FontResolver::new(book).resolve(&request).clone();
        let family = resolution
            .face()
            .and_then(|id| book.face(id))
            .and_then(|face| face.names().family().map(str::to_owned));
        (family, resolution.kind())
    }

    #[test]
    fn a_font_added_under_a_family_name_answers_to_it() {
        // A subset as Word embeds it, renamed, for text in "IBM Plex Serif Light".
        let renamed = TestFont::new("___WRD_EMBED_SUB_53").build();
        let mut book = FontBook::new();
        book.add_bytes(TestFont::new("Other Test").build(), FontOrigin::User)
            .unwrap();
        assert_eq!(
            book.add_bytes_as(
                renamed.clone(),
                FontOrigin::Document,
                "IBM Plex Serif Light"
            )
            .unwrap(),
            1
        );
        let found = Some("___WRD_EMBED_SUB_53".to_owned());
        for family in [
            "IBM Plex Serif Light",
            " ibm plex SERIF light ",
            "___WRD_EMBED_SUB_53",
        ] {
            assert_eq!(
                resolved(&book, FontRequest::new(family)),
                (found.clone(), ResolutionKind::Exact),
                "{family:?}"
            );
        }
        // Only the faces added with it answer to it.
        assert_eq!(book.faces[0].keys.families, ["other test"]);
        let mut book = FontBook::new();
        book.add_bytes(renamed.clone(), FontOrigin::Document)
            .unwrap();
        assert_eq!(
            resolved(&book, FontRequest::new("IBM Plex Serif Light")).1,
            ResolutionKind::Missing
        );
        // An empty name adds none, and the name is not added twice.
        for family in ["", "  ", "___wrd_embed_sub_53"] {
            let mut book = FontBook::new();
            book.add_bytes_as(renamed.clone(), FontOrigin::Document, family)
                .unwrap();
            assert_eq!(
                book.faces[0].keys.families,
                ["___wrd_embed_sub_53"],
                "{family:?}"
            );
        }
        // An error is that of `add_bytes`, and adds nothing.
        let mut book = FontBook::new();
        assert!(matches!(
            book.add_bytes_as(&b"not a font"[..], FontOrigin::Document, "Any"),
            Err(Error::NotAFont { .. })
        ));
        assert!(book.is_empty());
    }

    #[test]
    fn a_family_name_too_long_to_keep_or_one_a_face_has_adds_none() {
        let renamed = TestFont::new("___WRD_EMBED_SUB_7").build();
        // 512 bytes is the most the book keeps of a name.
        for (len, kept) in [(512, true), (513, false)] {
            let mut book = FontBook::new();
            book.add_bytes_as(renamed.clone(), FontOrigin::Document, &"a".repeat(len))
                .unwrap();
            assert_eq!(
                book.faces[0].keys.families.len(),
                1 + usize::from(kept),
                "{len}"
            );
        }
        // A collection of Alpha and Beta added under "Beta Test": Beta serves it, as it
        // would added plainly, and Alpha gets no name.
        let ttc = testing::collection(&[
            TestFont::new("Alpha Test").build(),
            TestFont::new("Beta Test").build(),
        ]);
        let mut book = FontBook::new();
        book.add_bytes_as(ttc, FontOrigin::Document, "Beta Test")
            .unwrap();
        assert_eq!(
            resolved(&book, FontRequest::new("Beta Test")).0.as_deref(),
            Some("Beta Test")
        );
        assert_eq!(book.faces[0].keys.families, ["alpha test"]);
    }

    #[test]
    fn a_family_name_goes_to_the_faces_that_answer_to_it_by_other_names() {
        // Word's subsets of Garamond: the regular face keeps its PostScript name, which is
        // the family's, the bold one "Garamond-Bold". Each gets the name, and regular text
        // finds the regular face, with nothing to report.
        let regular = TestFont::new("___WRD_EMBED_SUB_1")
            .postscript("Garamond")
            .build();
        let bold = TestFont::new("___WRD_EMBED_SUB_2")
            .style("Bold", 700, false)
            .postscript("Garamond-Bold")
            .build();
        let mut book = FontBook::new();
        book.add_bytes_as(regular.clone(), FontOrigin::Document, "Garamond")
            .unwrap();
        book.add_bytes_as(bold, FontOrigin::Document, "Garamond")
            .unwrap();
        let mut resolver = FontResolver::new(&book);
        for (bold, face) in [(false, "___WRD_EMBED_SUB_1"), (true, "___WRD_EMBED_SUB_2")] {
            let resolution = resolver
                .resolve(&FontRequest::new("Garamond").bold(bold))
                .clone();
            assert_eq!(resolution.kind(), ResolutionKind::Exact);
            let id = resolution.face().unwrap();
            assert_eq!(book.face(id).unwrap().names().family(), Some(face));
        }
        assert!(resolver.substitutions().is_empty());
        // The regular subset alone, with an installed Garamond: the document's face serves.
        let mut book = FontBook::new();
        book.add_bytes(TestFont::new("Garamond").build(), FontOrigin::User)
            .unwrap();
        book.add_bytes_as(regular, FontOrigin::Document, "Garamond")
            .unwrap();
        assert_eq!(
            resolved(&book, FontRequest::new("Garamond")),
            (Some("___WRD_EMBED_SUB_1".to_owned()), ResolutionKind::Exact)
        );
        // In a collection, only the face that answers by its typographic, full or
        // PostScript name gets it.
        let answering = TestFont::new("Gamma Test")
            .style("Bold", 700, false)
            .typographic("Plex Family", "Bold");
        for name in ["Plex Family", "Gamma Test Bold", "GammaTest-Bold"] {
            let ttc =
                testing::collection(&[answering.build(), TestFont::new("Delta Test").build()]);
            let mut book = FontBook::new();
            book.add_bytes_as(ttc, FontOrigin::Document, name).unwrap();
            assert_eq!(book.faces[0].keys.families.len(), 2, "{name}");
            assert_eq!(book.faces[1].keys.families, ["delta test"], "{name}");
            assert_eq!(
                resolved(&book, FontRequest::new(name)),
                (Some("Gamma Test".to_owned()), ResolutionKind::Exact),
                "{name}"
            );
        }
        // A face that has it as its family alone, not as its full or PostScript name,
        // keeps serving it, and gives the other face no name.
        let ttc = testing::collection(&[
            TestFont::new("Alpha Test").build(),
            TestFont::new("Beta Test").style("Bold", 700, false).build(),
        ]);
        let mut book = FontBook::new();
        book.add_bytes_as(ttc, FontOrigin::Document, "Beta Test")
            .unwrap();
        assert_eq!(book.faces[0].keys.families, ["alpha test"]);
        // Whichever face of the collection comes first.
        let ttc = testing::collection(&[TestFont::new("Delta Test").build(), answering.build()]);
        let mut book = FontBook::new();
        book.add_bytes_as(ttc, FontOrigin::Document, "GammaTest-Bold")
            .unwrap();
        assert_eq!(book.faces[0].keys.families, ["delta test"]);
        assert_eq!(book.faces[1].keys.families.len(), 2);
        // A variable font added under the name of one of its instances keeps it as one.
        let axes = [(b"wght", 300.0, 400.0, 700.0)];
        let instances: [(u16, &[f64]); 2] = [(256, &[400.0]), (257, &[600.0])];
        let variable = TestFont::new("Test Var")
            .name_record(256, "Regular")
            .name_record(257, "SemiBold")
            .with_table(b"fvar", testing::fvar(&axes, &instances))
            .build();
        let mut book = FontBook::new();
        book.add_bytes_as(variable, FontOrigin::Document, "Test Var SemiBold")
            .unwrap();
        assert_eq!(
            resolved(&book, FontRequest::new("Test Var SemiBold")).1,
            ResolutionKind::VariableInstance
        );
        // The bound is on the name as given, without the white space around it: 200
        // characters of two bytes, which lowercase to three, are kept, and 512 bytes with
        // spaces around them; 300 of three bytes, which lowercase to one, are not.
        for (name, kept) in [
            ("\u{23A}".repeat(200), true),
            (format!("  {}  ", "a".repeat(512)), true),
            ("\u{212A}".repeat(300), false),
        ] {
            let mut book = FontBook::new();
            book.add_bytes_as(
                TestFont::new("Epsilon Test").build(),
                FontOrigin::Document,
                &name,
            )
            .unwrap();
            assert_eq!(book.faces[0].keys.families.len(), 1 + usize::from(kept));
        }
    }

    #[test]
    fn a_font_file_added_under_a_family_name_answers_to_it() {
        let dir = TempDir::new("add-file-as");
        let path = dir.0.join("renamed.ttf");
        fs::write(&path, TestFont::new("___WRD_EMBED_SUB_9").build()).unwrap();
        let mut book = FontBook::new();
        assert_eq!(book.add_file_as(&path, "Plex Test").unwrap(), 1);
        assert_eq!(
            resolved(&book, FontRequest::new("Plex Test")),
            (Some("___WRD_EMBED_SUB_9".to_owned()), ResolutionKind::Exact)
        );
        // A file the book has read already adds no faces, and so no name.
        assert_eq!(book.add_file_as(&path, "Other Test").unwrap(), 0);
        assert_eq!(
            resolved(&book, FontRequest::new("Other Test")).1,
            ResolutionKind::Missing
        );
        assert!(matches!(
            book.add_file_as(dir.0.join("missing.ttf"), "Any"),
            Err(Error::Read { .. })
        ));
    }

    #[test]
    fn faces_added_under_a_family_name_keep_their_styles() {
        // A collection of a regular and a bold face, both renamed, and the document's
        // faces before a user's of the same family.
        let ttc = testing::collection(&[
            TestFont::new("___WRD_EMBED_SUB_1").build(),
            TestFont::new("___WRD_EMBED_SUB_2")
                .style("Bold", 700, false)
                .build(),
        ]);
        let mut book = FontBook::new();
        book.add_bytes(
            TestFont::new("Plex Test").style("Bold", 700, false).build(),
            FontOrigin::User,
        )
        .unwrap();
        assert_eq!(
            book.add_bytes_as(ttc, FontOrigin::Document, "Plex Test")
                .unwrap(),
            2
        );
        assert_eq!(
            resolved(&book, FontRequest::new("Plex Test")).0.as_deref(),
            Some("___WRD_EMBED_SUB_1")
        );
        assert_eq!(
            resolved(&book, FontRequest::new("Plex Test").bold(true))
                .0
                .as_deref(),
            Some("___WRD_EMBED_SUB_2")
        );
    }

    /// A fresh directory under the system temporary directory, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("officewright-font-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            // A test that fails midway can leave a directory without permission behind.
            #[cfg(unix)]
            permit(&self.0);
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Gives `dir` and the directories under it, not following links, back their
    /// permissions, so that they can be removed.
    #[cfg(unix)]
    fn permit(dir: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o755));
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_dir()) {
                permit(&path);
            }
        }
    }

    /// Makes `link` a symbolic link to the file `target`. On Windows this needs Developer
    /// Mode or an administrator, which CI runners have; without them the test fails, as
    /// CLAUDE.md allows no skipped tests.
    #[cfg(any(unix, windows))]
    fn file_link(target: &Path, link: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(target, link)
            .expect("a symbolic link needs Developer Mode or an administrator on Windows");
    }

    /// Makes `link` a link to the directory `target`: a symbolic link, and on Windows a
    /// directory junction, which needs no privilege.
    #[cfg(any(unix, windows))]
    fn dir_link(target: &Path, link: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        {
            let status = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .stdout(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "mklink /J {}", link.display());
        }
    }

    #[test]
    fn a_clone_is_a_book_of_its_own() {
        let dir = TempDir::new("clone");
        fs::write(dir.0.join("broken.ttf"), b"not a font").unwrap();
        let mut book = FontBook::new();
        book.add_dir(&dir.0).unwrap();
        let copy = book.clone();
        book.add_bytes(TestFont::new("Test Sans").build(), FontOrigin::User)
            .unwrap();
        assert_eq!((book.len(), copy.len()), (1, 0));
        assert_eq!(copy.skipped().len(), 1);
        assert_eq!(copy.skipped()[0].to_string(), book.skipped()[0].to_string());
    }

    #[test]
    fn indexes_fonts_from_memory() {
        let mut book = FontBook::new();
        let added = book
            .add_bytes(TestFont::new("Test Sans").build(), FontOrigin::Document)
            .unwrap();
        assert_eq!(added, 1);
        let (id, face) = book.faces().next().unwrap();
        assert_eq!(face.origin(), FontOrigin::Document);
        assert_eq!(face.path(), None);
        assert_eq!(face.names().family(), Some("Test Sans"));
        let font = book.load(id).unwrap();
        assert_eq!(font.names().family(), Some("Test Sans"));
    }

    #[test]
    fn rejects_memory_that_is_not_a_font() {
        let mut book = FontBook::new();
        let err = book
            .add_bytes(&b"not a font"[..], FontOrigin::User)
            .unwrap_err();
        assert!(matches!(err, Error::NotAFont { .. }), "{err}");
        assert!(book.is_empty());
    }

    #[test]
    fn scans_directories_recursively_and_skips_broken_files() {
        let dir = TempDir::new("scan");
        let nested = dir.0.join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.0.join("sans.TTF"), TestFont::new("Test Sans").build()).unwrap();
        let ttc = testing::collection(&[
            TestFont::new("Test Serif").build(),
            TestFont::new("Test Serif")
                .style("Bold", 700, false)
                .build(),
        ]);
        fs::write(nested.join("serif.ttc"), ttc).unwrap();
        fs::write(dir.0.join("broken.otf"), b"not a font").unwrap();
        fs::write(dir.0.join("readme.txt"), b"not a font either").unwrap();

        let mut book = FontBook::new();
        assert_eq!(book.add_dir(&dir.0).unwrap(), 3);
        let families: Vec<_> = book
            .faces()
            .map(|(_, face)| (face.names().family().unwrap(), face.index(), face.weight()))
            .collect();
        // A directory's own files come before its subdirectories.
        assert_eq!(
            families,
            [
                ("Test Sans", 0, 400),
                ("Test Serif", 0, 400),
                ("Test Serif", 1, 700)
            ]
        );
        assert_eq!(book.skipped().len(), 1);
        assert_eq!(
            book.skipped()[0].path(),
            Some(dir.0.join("broken.otf").as_path())
        );
        assert!(matches!(book.skipped()[0].error(), Error::NotAFont { .. }));
        // The same files are not added twice.
        assert_eq!(book.add_dir(&dir.0).unwrap(), 0);
        assert_eq!(book.len(), 3);
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn survives_directory_link_loops() {
        let dir = TempDir::new("loop");
        let (scan, fonts) = (dir.0.join("scan"), dir.0.join("fonts"));
        fs::create_dir(&scan).unwrap();
        fs::create_dir(&fonts).unwrap();
        fs::write(fonts.join("sans.ttf"), TestFont::new("Test Sans").build()).unwrap();
        // The font is reached only through a link, found inside the scanned folder;
        // three links in its folder lead back to it, so a scan that did not notice the
        // loop would read millions of folders before its depth limit.
        dir_link(&fonts, &scan.join("fonts"));
        for name in ["a", "b", "c"] {
            dir_link(&fonts, &fonts.join(name));
        }
        let mut book = FontBook::new();
        assert_eq!(book.add_dir(&scan).unwrap(), 1);
    }

    #[test]
    #[cfg(unix)]
    fn opens_only_regular_files() {
        // A FIFO or socket named like a font would block or fail when opened.
        let dir = TempDir::new("special");
        let socket = dir.0.join("socket.ttf");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        fs::write(dir.0.join("sans.ttf"), TestFont::new("Test Sans").build()).unwrap();
        let mut book = FontBook::new();
        assert_eq!(book.add_dir(&dir.0).unwrap(), 1);
        assert!(book.skipped().is_empty(), "{:?}", book.skipped());
        let err = book.add_file(&socket).unwrap_err();
        assert_eq!(err.to_string(), format!("cannot read {}", socket.display()));
        // A device opens fine but is not a font file either.
        let err = book.add_file("/dev/zero").unwrap_err();
        assert_eq!(
            std::error::Error::source(&err).map(ToString::to_string),
            Some("not a regular file".to_owned())
        );
    }

    #[test]
    fn broken_files_are_read_and_reported_once() {
        let dir = TempDir::new("once");
        fs::write(dir.0.join("broken.ttf"), b"not a font").unwrap();
        let mut book = FontBook::new();
        book.add_dir(&dir.0).unwrap();
        book.add_dir(&dir.0).unwrap();
        assert_eq!(book.skipped().len(), 1);
        assert_eq!(
            book.skipped()[0].to_string(),
            format!(
                "{}: data is not a TrueType or OpenType font or font collection: the file is \
                 too short",
                dir.0.join("broken.ttf").display()
            )
        );
    }

    /// Writes `data` to `path` and gives the file the modification time `modified`.
    fn write_with_time(path: &Path, data: &[u8], modified: SystemTime) {
        fs::write(path, data).unwrap();
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    #[test]
    fn reads_a_skipped_file_again_once_it_changes() {
        let dir = TempDir::new("retry");
        let path = dir.0.join("sans.ttf");
        let font = TestFont::new("Test Sans").build();
        // A copy still in progress: the font's size, but no font data yet.
        fs::write(&path, vec![0; font.len()]).unwrap();
        let mut book = FontBook::new();
        assert_eq!(book.add_dir(&dir.0).unwrap(), 0);
        assert_eq!(book.skipped().len(), 1);

        // Same size and modification time: the file is taken as unchanged, not read.
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        write_with_time(&path, &font, modified);
        assert_eq!(book.add_dir(&dir.0).unwrap(), 0);
        assert_eq!(book.skipped().len(), 1);

        // Once its modification time changes it is read, and it leaves the list.
        let later = modified + std::time::Duration::from_secs(2);
        write_with_time(&path, &font, later);
        assert_eq!(book.add_dir(&dir.0).unwrap(), 1);
        assert!(book.skipped().is_empty(), "{:?}", book.skipped());
        assert_eq!(book.faces().next().unwrap().1.path(), Some(path.as_path()));
    }

    #[test]
    fn reads_a_skipped_file_again_once_its_size_changes() {
        let dir = TempDir::new("size");
        let path = dir.0.join("sans.ttf");
        let font = TestFont::new("Test Sans").build();
        fs::write(&path, &font[..font.len() / 2]).unwrap();
        let mut book = FontBook::new();
        assert_eq!(book.add_dir(&dir.0).unwrap(), 0);
        assert_eq!(book.skipped().len(), 1);
        // Within the file system's timestamp resolution the time can stay the same.
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        write_with_time(&path, &font, modified);
        assert_eq!(book.add_dir(&dir.0).unwrap(), 1);
        assert!(book.skipped().is_empty(), "{:?}", book.skipped());
    }

    #[test]
    fn lists_a_skipped_file_once_with_its_latest_error() {
        let dir = TempDir::new("latest");
        let path = dir.0.join("sans.ttf");
        fs::write(&path, b"not a font").unwrap();
        let mut book = FontBook::new();
        book.add_dir(&dir.0).unwrap();
        assert!(matches!(book.skipped()[0].error(), Error::NotAFont { .. }));

        let damaged = TestFont::new("Test Sans").without_table(b"hmtx").build();
        fs::write(&path, damaged).unwrap();
        book.add_dir(&dir.0).unwrap();
        assert_eq!(book.skipped().len(), 1, "{:?}", book.skipped());
        assert!(
            matches!(
                book.skipped()[0].error(),
                Error::MissingTable { table: "hmtx", .. }
            ),
            "{}",
            book.skipped()[0]
        );
    }

    #[test]
    fn a_scan_lists_a_file_that_add_file_could_not_add() {
        let dir = TempDir::new("explicit");
        let path = dir.0.join("broken.ttf");
        fs::write(&path, b"not a font").unwrap();
        let mut book = FontBook::new();
        assert!(book.add_file(&path).is_err());
        assert!(book.skipped().is_empty());
        book.add_dir(&dir.0).unwrap();
        assert_eq!(book.skipped().len(), 1);
        assert_eq!(book.skipped()[0].path(), Some(path.as_path()));
        // add_file reads a skipped file again; once it is added it leaves the list.
        fs::write(&path, TestFont::new("Test Sans").build()).unwrap();
        assert_eq!(book.add_file(&path).unwrap(), 1);
        assert!(book.skipped().is_empty(), "{:?}", book.skipped());
    }

    #[test]
    #[cfg(unix)]
    fn adds_font_links_once_they_resolve() {
        let dir = TempDir::new("resolve");
        let fonts = dir.0.join("fonts");
        let elsewhere = dir.0.join("elsewhere");
        fs::create_dir(&fonts).unwrap();
        fs::create_dir(&elsewhere).unwrap();
        // A link to a font that is not there yet.
        let target = elsewhere.join("sans.ttf");
        std::os::unix::fs::symlink(&target, fonts.join("link.ttf")).unwrap();
        // A dangling link that a real font file replaces.
        let replaced = fonts.join("serif.ttf");
        std::os::unix::fs::symlink(elsewhere.join("removed.ttf"), &replaced).unwrap();

        let mut book = FontBook::new();
        assert_eq!(book.add_dir(&fonts).unwrap(), 0);
        assert_eq!(book.skipped().len(), 2, "{:?}", book.skipped());

        fs::write(&target, TestFont::new("Test Sans").build()).unwrap();
        fs::remove_file(&replaced).unwrap();
        fs::write(&replaced, TestFont::new("Test Serif").build()).unwrap();
        assert_eq!(book.add_dir(&fonts).unwrap(), 2);
        assert!(book.skipped().is_empty(), "{:?}", book.skipped());
    }

    #[test]
    #[cfg(unix)]
    fn a_broken_file_replaced_by_a_link_leaves_the_list_or_is_listed_once() {
        let dir = TempDir::new("relinked");
        let fonts = dir.0.join("fonts");
        let elsewhere = dir.0.join("elsewhere");
        fs::create_dir(&fonts).unwrap();
        fs::create_dir(&elsewhere).unwrap();
        let (good, bad) = (fonts.join("good.ttf"), fonts.join("bad.ttf"));
        fs::write(&good, b"not a font").unwrap();
        fs::write(&bad, b"not a font").unwrap();
        let mut book = FontBook::new();
        book.add_dir(&fonts).unwrap();
        assert_eq!(book.skipped().len(), 2);

        // A package upgrade replaces each file with a link into a shared directory: one
        // to a font, one to a file that is broken in another way.
        let font = elsewhere.join("sans.ttf");
        let damaged = elsewhere.join("damaged.ttf");
        fs::write(&font, TestFont::new("Test Sans").build()).unwrap();
        let without_hmtx = TestFont::new("Test Serif").without_table(b"hmtx").build();
        fs::write(&damaged, without_hmtx).unwrap();
        for (path, target) in [(&good, &font), (&bad, &damaged)] {
            fs::remove_file(path).unwrap();
            std::os::unix::fs::symlink(target, path).unwrap();
        }
        assert_eq!(book.add_dir(&fonts).unwrap(), 1);
        assert_eq!(book.skipped().len(), 1, "{:?}", book.skipped());
        assert_eq!(book.skipped()[0].path(), Some(bad.as_path()));
        assert!(
            matches!(
                book.skipped()[0].error(),
                Error::MissingTable { table: "hmtx", .. }
            ),
            "{}",
            book.skipped()[0]
        );
    }

    #[test]
    #[cfg(unix)]
    fn add_file_drops_a_link_that_a_scan_could_not_resolve() {
        let dir = TempDir::new("add-link");
        let fonts = dir.0.join("fonts");
        fs::create_dir(&fonts).unwrap();
        let target = dir.0.join("sans.ttf");
        std::os::unix::fs::symlink(&target, fonts.join("link.ttf")).unwrap();
        // The file is added through another path to the same directory.
        let alias = dir.0.join("alias");
        std::os::unix::fs::symlink(&fonts, &alias).unwrap();
        let mut book = FontBook::new();
        book.add_dir(&fonts).unwrap();
        assert_eq!(book.skipped().len(), 1);
        fs::write(&target, TestFont::new("Test Sans").build()).unwrap();
        assert_eq!(book.add_file(alias.join("link.ttf")).unwrap(), 1);
        assert!(book.skipped().is_empty(), "{:?}", book.skipped());
    }

    #[test]
    #[cfg(unix)]
    fn an_unresolvable_file_shows_the_error_of_its_latest_attempt() {
        let read_error = |skipped: &SkippedFont| match skipped.error() {
            Error::Read { source, .. } => source.kind(),
            error => panic!("{error}"),
        };
        let dir = TempDir::new("loop-link");
        let link = dir.0.join("sans.ttf");
        std::os::unix::fs::symlink(dir.0.join("removed.ttf"), &link).unwrap();
        let mut book = FontBook::new();
        book.add_dir(&dir.0).unwrap();
        assert_eq!(read_error(&book.skipped()[0]), io::ErrorKind::NotFound);

        // Now a link to itself, which cannot be resolved for another reason.
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&link, &link).unwrap();
        book.add_dir(&dir.0).unwrap();
        assert_eq!(book.skipped().len(), 1, "{:?}", book.skipped());
        assert_eq!(book.skipped()[0].path(), Some(link.as_path()));
        assert_ne!(read_error(&book.skipped()[0]), io::ErrorKind::NotFound);
    }

    #[test]
    #[cfg(unix)]
    fn lists_an_unreadable_directory_once_and_reads_it_when_it_can() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new("locked");
        let locked = dir.0.join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("sans.ttf"), TestFont::new("Test Sans").build()).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let readable = fs::read_dir(&locked).is_ok();
        let mut book = FontBook::new();
        let scans = [book.add_dir(&dir.0), book.add_dir(&dir.0)];
        let skipped = book.skipped().len();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        // Root reads every directory, so this has to run as an ordinary user.
        assert!(
            !readable,
            "run the tests as a user who cannot read mode 000"
        );
        assert_eq!(scans.map(Result::unwrap), [0, 0]);
        assert_eq!(skipped, 1);

        assert_eq!(book.add_dir(&dir.0).unwrap(), 1);
        assert!(book.skipped().is_empty(), "{:?}", book.skipped());
    }

    #[test]
    #[cfg(unix)]
    fn an_unreadable_directory_leaves_the_list_once_read_through_a_link() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let set_mode =
            |path: &Path, mode| fs::set_permissions(path, fs::Permissions::from_mode(mode));
        let dir = TempDir::new("relinked-dir");
        let (fonts, locked, other) = (
            dir.0.join("fonts"),
            dir.0.join("locked"),
            dir.0.join("other"),
        );
        for path in [&fonts, &locked, &other] {
            fs::create_dir(path).unwrap();
        }
        fs::write(other.join("sans.ttf"), TestFont::new("Test Sans").build()).unwrap();
        // Root reads every directory, so this has to run as an ordinary user.
        set_mode(&locked, 0o000).unwrap();
        let readable = fs::read_dir(&locked).is_ok();
        set_mode(&locked, 0o755).unwrap();
        assert!(
            !readable,
            "run the tests as a user who cannot read mode 000"
        );
        // `fonts` reached through a link to it, to name a root another way.
        let fonts_link = dir.0.join("fonts-link");
        symlink(&fonts, &fonts_link).unwrap();
        let (sub, alias) = (fonts.join("sub"), fonts.join("alias"));
        let scan_fonts = |book: &mut FontBook| book.add_dir(&fonts).unwrap();
        // As the root of a scan, as the system font directories are.
        let scan_sub = |book: &mut FontBook| book.scan_dir(&sub, FontOrigin::User);
        let scan_sub_linked =
            |book: &mut FontBook| book.scan_dir(&fonts_link.join("sub"), FontOrigin::User);
        let scan_alias = |book: &mut FontBook| book.scan_dir(&alias, FontOrigin::User);
        let scan_locked = |book: &mut FontBook| book.scan_dir(&locked, FontOrigin::User);
        let unreadable = || {
            fs::create_dir(&sub).unwrap();
            set_mode(&sub, 0o000).unwrap();
        };
        let link_to_locked = || {
            set_mode(&locked, 0o000).unwrap();
            symlink(&locked, &sub).unwrap();
        };
        let relink = || {
            if fs::symlink_metadata(&sub).unwrap().is_dir() {
                fs::remove_dir(&sub).unwrap();
            } else {
                fs::remove_file(&sub).unwrap();
            }
            symlink(&other, &sub).unwrap();
        };
        let unlock_and_alias = || {
            set_mode(&sub, 0o755).unwrap();
            symlink(&sub, &alias).unwrap();
        };
        let unlock_locked = || set_mode(&locked, 0o755).unwrap();
        // A link `fonts/alias` to `other`, which a scan of `fonts` reads before `sub`.
        let unreadable_beside_alias = || {
            symlink(&other, &alias).unwrap();
            unreadable();
        };
        let alias_and_relink = || {
            symlink(&other, &alias).unwrap();
            relink();
        };
        let relink_to_root = || {
            fs::remove_dir(&sub).unwrap();
            symlink(&fonts, &sub).unwrap();
        };
        // `alias` comes before `sub`, so a scan of `fonts` reads `locked` through it and
        // skips `sub` as a directory it has read.
        let unlock_locked_and_alias = || {
            set_mode(&locked, 0o755).unwrap();
            symlink(&locked, &alias).unwrap();
        };
        type Scan<'a> = &'a dyn Fn(&mut FontBook) -> usize;
        // A case: its name, how the directory is made, the first scan, the change, the
        // scan after it, and the faces that scan adds (Test Sans, through a link to
        // `other`, or none).
        type Case<'a> = (
            &'a str,
            &'a dyn Fn(),
            Scan<'a>,
            &'a dyn Fn(),
            Scan<'a>,
            usize,
        );
        let cases: [Case<'_>; 10] = [
            // A directory without permission, replaced by a link to a readable one,
            // reached as a subdirectory, as a root, and as a root named another way.
            (
                "subdirectory",
                &unreadable,
                &scan_fonts,
                &relink,
                &scan_fonts,
                1,
            ),
            ("root", &unreadable, &scan_sub, &relink, &scan_sub, 1),
            (
                "root by another path",
                &unreadable,
                &scan_sub,
                &relink,
                &scan_sub_linked,
                1,
            ),
            // A link to a directory without permission, pointed at a readable one.
            (
                "link",
                &link_to_locked,
                &scan_fonts,
                &relink,
                &scan_fonts,
                1,
            ),
            // A directory without permission that becomes readable, read through
            // another link to it: a link to `fonts/sub`, or one to the directory behind
            // the link `fonts/sub`, or that directory as a root.
            (
                "alias",
                &unreadable,
                &scan_fonts,
                &unlock_and_alias,
                &scan_alias,
                0,
            ),
            (
                "other link",
                &link_to_locked,
                &scan_fonts,
                &unlock_locked_and_alias,
                &scan_fonts,
                0,
            ),
            // Replaced by a link to a directory the scan has read before it, through
            // another link, or the root itself; or repointed to one.
            (
                "relinked to a directory read first",
                &unreadable_beside_alias,
                &scan_fonts,
                &relink,
                &scan_fonts,
                0,
            ),
            (
                "repointed to a directory read first",
                &link_to_locked,
                &scan_fonts,
                &alias_and_relink,
                &scan_fonts,
                1,
            ),
            (
                "relinked to the root",
                &unreadable,
                &scan_fonts,
                &relink_to_root,
                &scan_fonts,
                0,
            ),
            (
                "target",
                &link_to_locked,
                &scan_fonts,
                &unlock_locked,
                &scan_locked,
                0,
            ),
        ];
        // For each case: what its book listed after the first scan, and what it listed
        // and how many faces it added after the second.
        let mut results = Vec::new();
        for (name, before, first, change, second, faces) in cases {
            before();
            let mut book = FontBook::new();
            first(&mut book);
            let listed: Vec<_> = book
                .skipped()
                .iter()
                .map(|s| s.path().map(Path::to_owned))
                .collect();
            change();
            let added = second(&mut book);
            results.push((name, listed, added, faces, book.skipped().to_vec()));
            // The next case starts from an empty directory of fonts.
            for path in [&alias, &sub] {
                match fs::symlink_metadata(path) {
                    Ok(metadata) if metadata.is_dir() => {
                        set_mode(path, 0o755).unwrap();
                        fs::remove_dir(path).unwrap();
                    }
                    Ok(_) => fs::remove_file(path).unwrap(),
                    Err(_) => {}
                }
            }
        }
        set_mode(&locked, 0o755).unwrap();

        for (name, listed, added, faces, skipped) in results {
            assert_eq!(listed, [Some(sub.clone())], "{name}");
            assert_eq!(added, faces, "{name}");
            assert!(skipped.is_empty(), "{name}: {skipped:?}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn keeps_an_unreadable_directory_listed_while_others_are_read() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new("still-locked");
        let (one, two) = (dir.0.join("one"), dir.0.join("two"));
        let locked = one.join("sub");
        fs::create_dir_all(&locked).unwrap();
        fs::create_dir(&two).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let mut book = FontBook::new();
        book.add_dir(&one).unwrap();
        book.add_dir(&two).unwrap();
        let skipped = book.skipped().len();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        // Reading `two` says nothing about `one/sub`, which still cannot be read.
        assert_eq!(skipped, 1);
    }

    #[test]
    #[cfg(unix)]
    fn lists_an_unreadable_directory_once_per_scan_under_each_link() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let dir = TempDir::new("two-links");
        let (fonts, locked) = (dir.0.join("fonts"), dir.0.join("locked"));
        fs::create_dir(&fonts).unwrap();
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        symlink(&locked, fonts.join("b")).unwrap();
        let mut book = FontBook::new();
        book.add_dir(&fonts).unwrap();
        // A second link to the same directory, which the scans reach first.
        symlink(&locked, fonts.join("a")).unwrap();
        book.add_dir(&fonts).unwrap();
        let mut fresh = FontBook::new();
        fresh.add_dir(&fonts).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        let paths = |book: &FontBook| -> Vec<PathBuf> {
            book.skipped()
                .iter()
                .filter_map(|skipped| skipped.path().map(Path::to_owned))
                .collect()
        };
        // Across scans, under each link reached; in one scan, once, under the first.
        assert_eq!(paths(&book), [fonts.join("b"), fonts.join("a")]);
        assert_eq!(paths(&fresh), [fonts.join("a")]);
    }

    #[test]
    #[cfg(unix)]
    fn a_link_repointed_follows_its_new_target() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let set_mode =
            |path: &Path, mode| fs::set_permissions(path, fs::Permissions::from_mode(mode));
        let dir = TempDir::new("repointed");
        let (fonts, first, second) = (dir.0.join("fonts"), dir.0.join("t1"), dir.0.join("t2"));
        for path in [&fonts, &first, &second] {
            fs::create_dir(path).unwrap();
        }
        fs::write(second.join("sans.ttf"), TestFont::new("Test Sans").build()).unwrap();
        set_mode(&first, 0o000).unwrap();
        set_mode(&second, 0o000).unwrap();
        // `a` leads to `t2` and `b` to `t1`, neither readable.
        symlink(&second, fonts.join("a")).unwrap();
        symlink(&first, fonts.join("b")).unwrap();
        let mut book = FontBook::new();
        book.add_dir(&fonts).unwrap();
        let listed = book.skipped().len();
        // `b` now leads to `t2`, which the next scan reaches first through `a`, so it
        // skips `b` but remembers where it leads. Then `t2` becomes readable, and reading
        // it as a root, which reaches neither link, drops both entries.
        fs::remove_file(fonts.join("b")).unwrap();
        symlink(&second, fonts.join("b")).unwrap();
        book.add_dir(&fonts).unwrap();
        set_mode(&second, 0o755).unwrap();
        let added = book.add_dir(&second).unwrap();
        let skipped = book.skipped().to_vec();
        set_mode(&first, 0o755).unwrap();
        assert_eq!(listed, 2);
        assert_eq!(added, 1);
        assert!(skipped.is_empty(), "{skipped:?}");
    }

    #[test]
    #[cfg(unix)]
    fn a_link_repointed_to_another_unreadable_directory_stays_listed() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let set_mode =
            |path: &Path, mode| fs::set_permissions(path, fs::Permissions::from_mode(mode));
        let paths = |book: &FontBook| -> Vec<PathBuf> {
            book.skipped()
                .iter()
                .filter_map(|skipped| skipped.path().map(Path::to_owned))
                .collect()
        };
        let dir = TempDir::new("repointed-locked");
        let (fonts, first, second) = (dir.0.join("fonts"), dir.0.join("t1"), dir.0.join("t2"));
        for path in [&fonts, &first, &second] {
            fs::create_dir(path).unwrap();
        }
        set_mode(&first, 0o000).unwrap();
        set_mode(&second, 0o000).unwrap();
        let link = fonts.join("l");
        symlink(&first, &link).unwrap();
        let mut book = FontBook::new();
        book.add_dir(&fonts).unwrap();
        // `l` now leads to `t2`, which cannot be read either; `t1` can be read now, and is
        // read as a root, which does not reach `l`.
        fs::remove_file(&link).unwrap();
        symlink(&second, &link).unwrap();
        set_mode(&first, 0o755).unwrap();
        book.add_dir(&first).unwrap();
        let after_root = paths(&book);
        // Once `t2` can be read, reading it as a root drops `l`, which leads there now.
        set_mode(&second, 0o755).unwrap();
        book.add_dir(&second).unwrap();
        let after_target = paths(&book);

        // The same within one scan: `1` leads to `t2` and `2` to `t1`, both reached before
        // `l`; reading `t1` through `2` keeps `l`, which leads to `t2`.
        let dir = TempDir::new("repointed-in-scan");
        let (fonts, first, second) = (dir.0.join("fonts"), dir.0.join("t1"), dir.0.join("t2"));
        for path in [&fonts, &first, &second] {
            fs::create_dir(path).unwrap();
        }
        set_mode(&first, 0o000).unwrap();
        set_mode(&second, 0o000).unwrap();
        let in_scan = fonts.join("l");
        symlink(&first, &in_scan).unwrap();
        let mut scanned = FontBook::new();
        scanned.add_dir(&fonts).unwrap();
        fs::remove_file(&in_scan).unwrap();
        symlink(&second, &in_scan).unwrap();
        symlink(&second, fonts.join("1")).unwrap();
        symlink(&first, fonts.join("2")).unwrap();
        set_mode(&first, 0o755).unwrap();
        scanned.add_dir(&fonts).unwrap();
        let after_scan = paths(&scanned);
        set_mode(&second, 0o755).unwrap();

        assert_eq!(after_root, [link]);
        assert!(after_target.is_empty(), "{after_target:?}");
        assert_eq!(after_scan, [in_scan, fonts.join("1")]);
    }

    #[test]
    #[cfg(unix)]
    fn a_listed_link_that_no_longer_leads_to_an_unreadable_directory_leaves_the_list() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let set_mode =
            |path: &Path, mode| fs::set_permissions(path, fs::Permissions::from_mode(mode));
        // `fonts/l` is listed as leading to `t1`, which cannot be read; then it leads to
        // the directory `t3`, which can, to the file `f`, or nowhere, and `t1` becomes
        // readable. However `t1` is then read, a new book would not list `l`, and neither
        // does this one.
        type Read<'a> = &'a dyn Fn(&mut FontBook, &Path);
        let roots = |book: &mut FontBook, dir: &Path, first: &Path, second: &Path| {
            let other = dir.join("other");
            fs::create_dir(&other).unwrap();
            symlink(first, other.join("a")).unwrap();
            symlink(second, other.join("b")).unwrap();
            book.add_dir(&other).unwrap();
        };
        let cases: [(&str, &str, Read<'_>); 5] = [
            ("t3", "t3 then t1 in one scan", &|book, dir| {
                roots(book, dir, &dir.join("t3"), &dir.join("t1"));
            }),
            ("t3", "t1 then t3 in one scan", &|book, dir| {
                roots(book, dir, &dir.join("t1"), &dir.join("t3"));
            }),
            ("t3", "t3 then t1 as roots", &|book, dir| {
                book.add_dir(dir.join("t3")).unwrap();
                book.add_dir(dir.join("t1")).unwrap();
            }),
            ("f", "t1 as a root", &|book, dir| {
                book.add_dir(dir.join("t1")).unwrap();
            }),
            ("gone", "t1 as a root", &|book, dir| {
                book.add_dir(dir.join("t1")).unwrap();
            }),
        ];
        for (index, (target, name, read)) in cases.into_iter().enumerate() {
            let dir = TempDir::new(&format!("no-longer-locked-{index}"));
            let (fonts, first) = (dir.0.join("fonts"), dir.0.join("t1"));
            for path in [&fonts, &first, &dir.0.join("t3")] {
                fs::create_dir(path).unwrap();
            }
            fs::write(dir.0.join("f"), b"").unwrap();
            set_mode(&first, 0o000).unwrap();
            let link = fonts.join("l");
            symlink(&first, &link).unwrap();
            let mut book = FontBook::new();
            book.add_dir(&fonts).unwrap();
            let listed = book.skipped().len();
            fs::remove_file(&link).unwrap();
            symlink(dir.0.join(target), &link).unwrap();
            set_mode(&first, 0o755).unwrap();
            read(&mut book, &dir.0);
            assert_eq!(listed, 1, "{target}, {name}");
            assert!(
                book.skipped().is_empty(),
                "{target}, {name}: {:?}",
                book.skipped()
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn a_directory_link_that_becomes_a_dangling_font_link_stays_listed() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let set_mode =
            |path: &Path, mode| fs::set_permissions(path, fs::Permissions::from_mode(mode));
        let dir = TempDir::new("dir-to-font-link");
        let (fonts, locked) = (dir.0.join("fonts"), dir.0.join("locked"));
        fs::create_dir(&fonts).unwrap();
        fs::create_dir(&locked).unwrap();
        set_mode(&locked, 0o000).unwrap();
        // A link named like a font file, leading to a directory that cannot be read.
        let link = fonts.join("x.ttf");
        symlink(&locked, &link).unwrap();
        let mut book = FontBook::new();
        book.add_dir(&fonts).unwrap();
        // It becomes a link to a removed font file, and stays listed as one; reading
        // the directory it led to does not drop it.
        fs::remove_file(&link).unwrap();
        symlink(dir.0.join("gone.ttf"), &link).unwrap();
        book.add_dir(&fonts).unwrap();
        set_mode(&locked, 0o755).unwrap();
        book.add_dir(&locked).unwrap();
        assert_eq!(book.skipped().len(), 1, "{:?}", book.skipped());
        assert_eq!(book.skipped()[0].path(), Some(link.as_path()));
    }

    #[test]
    fn descends_at_most_16_directories() {
        let dir = TempDir::new("depth");
        let mut deepest = dir.0.clone();
        for level in 1..=17 {
            deepest = deepest.join(level.to_string());
            fs::create_dir(&deepest).unwrap();
            if level >= 16 {
                let font = TestFont::new(&format!("Depth {level}")).build();
                fs::write(deepest.join("font.ttf"), font).unwrap();
            }
        }
        let mut book = FontBook::new();
        assert_eq!(book.add_dir(&dir.0).unwrap(), 1);
        let (_, face) = book.faces().next().unwrap();
        assert_eq!(face.names().family(), Some("Depth 16"));
    }

    #[test]
    fn a_collection_with_only_damaged_faces_is_an_error() {
        let mut ttc = testing::collection(&[
            TestFont::new("Test Sans").build(),
            TestFont::new("Test Serif").build(),
        ]);
        // Point both faces past the end of the file.
        ttc[12..20].fill(0xFF);
        let mut book = FontBook::new();
        let err = book.add_bytes(ttc, FontOrigin::User).unwrap_err();
        assert!(matches!(err, Error::MalformedFace { face: 0, .. }), "{err}");
        assert!(book.is_empty());
    }

    #[test]
    fn reports_unreadable_directories_and_files() {
        let mut book = FontBook::new();
        let missing = std::env::temp_dir().join("officewright-font-no-such-dir");
        assert!(matches!(book.add_dir(&missing), Err(Error::Read { .. })));
        let file = missing.join("x.ttf");
        let err = book.add_file(&file).unwrap_err();
        assert!(matches!(err, Error::Read { .. }), "{err}");
        let skipped = SkippedFont {
            path: Some(file.clone()),
            error: Arc::new(err),
            key: None,
        };
        // The error names the file already; the I/O error says why it failed.
        let text = skipped.to_string();
        let prefix = format!("cannot read {}: ", file.display());
        assert!(
            text.starts_with(&prefix) && text.len() > prefix.len(),
            "{text}"
        );
    }

    #[test]
    fn skipped_fonts_say_why() {
        let budget = SkippedFont {
            path: None,
            error: Arc::new(Error::MalformedFace {
                face: 64,
                source: crate::error::Cause::new("faces 64 to 1023 were not read"),
            }),
            key: None,
        };
        assert_eq!(
            budget.to_string(),
            "font data: font face 64: the table directory is damaged: faces 64 to 1023 were \
             not read"
        );
        let read = SkippedFont {
            path: Some(PathBuf::from("a.ttf")),
            error: Arc::new(Error::Read {
                path: PathBuf::from("a.ttf"),
                source: io::Error::other("disk gone"),
            }),
            key: None,
        };
        assert_eq!(read.to_string(), "cannot read a.ttf: disk gone");
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn adds_fonts_through_links() {
        let dir = TempDir::new("links");
        let fonts = dir.0.join("fonts");
        let links = dir.0.join("links");
        fs::create_dir_all(&fonts).unwrap();
        fs::create_dir_all(&links).unwrap();
        let font = fonts.join("sans.ttf");
        let link = links.join("link.ttf");
        fs::write(&font, TestFont::new("Test Sans").build()).unwrap();
        file_link(&font, &link);

        let mut book = FontBook::new();
        assert_eq!(book.add_file(&link).unwrap(), 1);
        assert_eq!(book.faces().next().unwrap().1.path(), Some(link.as_path()));
        // The link and its target are the same file.
        assert_eq!(book.add_file(&font).unwrap(), 0);

        let mut book = FontBook::new();
        assert_eq!(book.add_dir(&links).unwrap(), 1);
        assert!(book.skipped().is_empty(), "{:?}", book.skipped());
        // The folder of fonts reached through a link to it holds the same font.
        let alias = dir.0.join("alias");
        dir_link(&fonts, &alias);
        assert_eq!(book.add_dir(&alias).unwrap(), 0);
        assert_eq!(book.add_dir(&fonts).unwrap(), 0);
        assert_eq!(book.len(), 1);
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn lists_font_files_that_cannot_be_resolved_once() {
        let dir = TempDir::new("dangling");
        let fonts = dir.0.join("fonts");
        fs::create_dir(&fonts).unwrap();
        let link = fonts.join("gone.ttf");
        file_link(&fonts.join("removed.ttf"), &link);
        // A dangling link with another name is not a font file and is not listed.
        file_link(&fonts.join("removed.txt"), &fonts.join("gone.txt"));
        // The same directory reached through a link to it.
        let alias = dir.0.join("alias");
        dir_link(&fonts, &alias);
        let mut book = FontBook::new();
        book.add_dir(&fonts).unwrap();
        book.add_dir(&fonts).unwrap();
        book.add_dir(&alias).unwrap();
        // On Windows, a directory symbolic link reaches it too, as a junction does.
        #[cfg(windows)]
        {
            let symlink = dir.0.join("symlink");
            std::os::windows::fs::symlink_dir(&fonts, &symlink)
                .expect("a symbolic link needs Developer Mode or an administrator on Windows");
            book.add_dir(&symlink).unwrap();
        }
        assert_eq!(book.skipped().len(), 1, "{:?}", book.skipped());
        assert_eq!(book.skipped()[0].path(), Some(link.as_path()));
        assert!(matches!(book.skipped()[0].error(), Error::Read { .. }));
        // The message names the same path, not the alias it was last reached through.
        let message = book.skipped()[0].to_string();
        let prefix = format!("cannot read {}: ", link.display());
        assert!(message.starts_with(&prefix), "{message}");
    }

    #[test]
    fn load_reports_ids_from_another_book() {
        let book = FontBook::new();
        assert!(matches!(
            book.load(FaceId(3)),
            Err(Error::FaceIndexOutOfRange { index: 3, count: 0 })
        ));
    }

    /// The family names of `ids`, in order.
    fn families(book: &FontBook, ids: &[FaceId]) -> Vec<String> {
        ids.iter()
            .map(|&id| book.face(id).unwrap().names().family().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn finds_the_faces_that_cover_a_character_in_book_order() {
        let mut book = FontBook::new();
        for font in [
            TestFont::new("Test Symbol").symbol(),
            TestFont::new("Test Sans"),
            TestFont::new("Test Serif"),
        ] {
            book.add_bytes(font.build(), FontOrigin::User).unwrap();
        }
        // The test fonts map the space, A, U+00B4 and U+00C1 (at U+F0xx in the symbol
        // font).
        for ch in [' ', 'A', '\u{B4}', '\u{C1}'] {
            let covering = book.faces_covering(ch);
            assert_eq!(
                families(&book, &covering),
                ["Test Sans", "Test Serif"],
                "{ch:?}"
            );
        }
        for ch in ['\u{F020}', '\u{F041}', '\u{F0C1}'] {
            let covering = book.faces_covering(ch);
            assert_eq!(families(&book, &covering), ["Test Symbol"], "{ch:?}");
        }
        // The symbol font's U+0020 to U+00FF alias does not count, though glyph_id finds
        // it, as Windows does.
        let symbol = book.load(FaceId(0)).unwrap();
        assert!(symbol.glyph_id('A').is_some());
        for ch in ['B', '\u{4E00}', '\u{F042}', '\0', char::MAX] {
            assert!(book.faces_covering(ch).is_empty(), "{ch:?}");
        }
    }

    #[test]
    fn covers_per_face_of_a_collection() {
        let ttc = testing::collection(&[
            TestFont::new("Test Symbol").symbol().build(),
            TestFont::new("Test Sans").build(),
        ]);
        let mut book = FontBook::new();
        book.add_bytes(ttc, FontOrigin::User).unwrap();
        assert_eq!(families(&book, &book.faces_covering('A')), ["Test Sans"]);
        assert_eq!(
            families(&book, &book.faces_covering('\u{F041}')),
            ["Test Symbol"]
        );
    }

    #[test]
    fn a_damaged_character_map_covers_nothing() {
        let mut book = FontBook::new();
        // The index does not read cmap, so the face is added.
        let damaged = TestFont::new("Test Damaged").with_table(b"cmap", vec![0, 0, 0, 9, 0xFF]);
        book.add_bytes(damaged.build(), FontOrigin::User).unwrap();
        book.add_bytes(TestFont::new("Test Sans").build(), FontOrigin::User)
            .unwrap();
        assert_eq!(families(&book, &book.faces_covering('A')), ["Test Sans"]);
        // A face without cmap covers nothing either.
        let mut book = FontBook::new();
        let bare = TestFont::new("Test Bare").without_table(b"cmap");
        book.add_bytes(bare.build(), FontOrigin::User).unwrap();
        assert!(book.faces_covering('A').is_empty());
    }

    #[test]
    fn a_file_that_went_away_covers_nothing_and_is_not_read_again() {
        let dir = TempDir::new("coverage-gone");
        let path = dir.0.join("sans.ttf");
        let font = TestFont::new("Test Sans").build();
        fs::write(&path, &font).unwrap();
        let mut book = FontBook::new();
        book.add_dir(&dir.0).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(book.faces_covering('A').is_empty());
        // Back again: the book keeps what the first query found.
        fs::write(&path, &font).unwrap();
        assert!(book.faces_covering('A').is_empty());
        // A new book reads it.
        let mut fresh = FontBook::new();
        fresh.add_dir(&dir.0).unwrap();
        assert_eq!(fresh.faces_covering('A').len(), 1);
    }

    #[test]
    fn coverage_is_read_once_and_survives_a_rescan() {
        let dir = TempDir::new("coverage-rescan");
        fs::write(dir.0.join("a.ttf"), TestFont::new("Test Sans").build()).unwrap();
        let mut book = FontBook::new();
        book.add_dir(&dir.0).unwrap();
        assert_eq!(families(&book, &book.faces_covering('A')), ["Test Sans"]);
        // A font installed since, and the first one changed on disk: a book does not read
        // an added file again, its coverage included.
        fs::write(dir.0.join("b.ttf"), TestFont::new("Test Serif").build()).unwrap();
        fs::write(
            dir.0.join("a.ttf"),
            TestFont::new("Test Sans").symbol().build(),
        )
        .unwrap();
        assert_eq!(book.add_dir(&dir.0).unwrap(), 1);
        assert_eq!(
            families(&book, &book.faces_covering('A')),
            ["Test Sans", "Test Serif"]
        );
    }

    #[test]
    fn clones_and_threads_get_the_same_answers() {
        fn send_and_sync<T: Send + Sync>() {}
        send_and_sync::<FontBook>();
        let mut book = FontBook::new();
        book.add_bytes(TestFont::new("Test Sans").build(), FontOrigin::User)
            .unwrap();
        book.add_bytes(
            TestFont::new("Test Symbol").symbol().build(),
            FontOrigin::User,
        )
        .unwrap();
        let copy = book.clone();
        let answers: Vec<Vec<FaceId>> = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..8)
                .map(|i| {
                    let book = if i % 2 == 0 { &book } else { &copy };
                    scope.spawn(move || book.faces_covering('A'))
                })
                .collect();
            threads.into_iter().map(|t| t.join().unwrap()).collect()
        });
        assert!(
            answers.iter().all(|answer| *answer == [FaceId(0)]),
            "{answers:?}"
        );
        assert_eq!(copy.faces_covering('\u{F041}'), [FaceId(1)]);
        // What was read is shown as a count, not as the ranges themselves.
        let debug = format!("{:?}", book.face(FaceId(0)).unwrap());
        assert!(debug.contains("coverage: 4 ranges"), "{debug}");
    }

    #[test]
    fn covers_only_glyphs_the_font_has_and_lookups_find() {
        let delta = |code: u16, glyph: u16| glyph.wrapping_sub(code);
        let mut book = FontBook::new();
        // B maps to glyph 99, past the end of the five glyphs the test font has.
        let past_the_end =
            testing::cmap_format4(&[(0x41, 0x41, delta(0x41, 2)), (0x42, 0x42, delta(0x42, 99))]);
        // Segments out of order: the subtable lists A, but a lookup, a binary search,
        // does not find it.
        let out_of_order =
            testing::cmap_format4(&[(0x41, 0x41, delta(0x41, 2)), (0x10, 0x10, delta(0x10, 1))]);
        for (family, cmap) in [("Test Past", past_the_end), ("Test Order", out_of_order)] {
            book.add_bytes(
                TestFont::new(family).with_table(b"cmap", cmap).build(),
                FontOrigin::User,
            )
            .unwrap();
        }
        let past = book.load(FaceId(0)).unwrap();
        assert!(past.glyph_id('A').is_some() && past.glyph_id('B').is_none());
        let order = book.load(FaceId(1)).unwrap();
        assert!(order.glyph_id('\u{10}').is_some() && order.glyph_id('A').is_none());
        assert_eq!(families(&book, &book.faces_covering('A')), ["Test Past"]);
        assert!(book.faces_covering('B').is_empty());
    }

    #[test]
    fn coverage_agrees_with_lookups_for_every_kind_of_subtable() {
        use testing::{cmap_of, format4, groups_subtable};
        let d = |code: u16, glyph: u16| glyph.wrapping_sub(code);
        // The glyph ID array index where segment `i` of `count` (the closing one
        // included) starts reading, as an idRangeOffset.
        let at = |i: u16, count: u16, index: u16| 2 * (count - i + index);
        let array = [0, 1, 2, 9, 3, 0, 4, 1, 2, 3, 4, 5, 0, 1, 2, 3];
        let cases: [(&str, Vec<u8>); 10] = [
            (
                "glyph array",
                cmap_of(&[(
                    3,
                    1,
                    format4(
                        &[
                            (0x20, 0x2F, 0, at(0, 5, 0)),
                            (0x100, 0x107, 0xFFFF, at(1, 5, 4)),
                            (0x200, 0x2FF, 0, at(2, 5, 10)),
                            // An entry of 0 maps nothing, whatever the delta.
                            (0x300, 0x303, 2, at(3, 5, 0)),
                        ],
                        &array,
                    ),
                )]),
            ),
            (
                "a repeated closing segment",
                cmap_of(&[(
                    3,
                    1,
                    format4(&[(0x41, 0x43, d(0x41, 1), 0), (0xFFFF, 0xFFFF, 1, 0)], &[]),
                )]),
            ),
            (
                "groups past the end of Unicode",
                testing::cmap_groups(13, &[(0x41, 0x41, 2), (0x11_0000, 0x11_0010, 2)]),
            ),
            (
                "deltas that wrap",
                testing::cmap_format4(&[(0x10, 0x20, 0xFFEE), (0xFFF0, 0xFFFE, 0x12)]),
            ),
            (
                "format 12",
                testing::cmap_groups(
                    12,
                    &[
                        (0x1000, 0x1010, u32::MAX - 2),
                        // Glyphs from 65,536 on: none of them in the font.
                        (0x3000, 0x3010, 0x1_0000),
                        (0x2_0000, 0x2_0003, 1),
                        (0x10_FFF0, 0x10_FFFF, 1),
                    ],
                ),
            ),
            (
                "format 13",
                testing::cmap_groups(
                    13,
                    &[
                        (0x41, 0x45, 0),
                        (0x50, 0x60, 3),
                        (0x1_F600, 0x1_F64F, 4),
                        (0x2_0000, 0x2_0001, 7),
                    ],
                ),
            ),
            (
                "full Unicode over BMP",
                // The full subtable comes first, so it wins only by its kind.
                cmap_of(&[
                    (3, 10, groups_subtable(12, &[(0x42, 0x42, 3)])),
                    (0, 3, format4(&[(0x41, 0x41, d(0x41, 2), 0)], &[])),
                ]),
            ),
            (
                "the last of a kind",
                cmap_of(&[
                    (3, 1, format4(&[(0x41, 0x41, d(0x41, 2), 0)], &[])),
                    (0, 3, format4(&[(0x42, 0x42, d(0x42, 3), 0)], &[])),
                ]),
            ),
            (
                "records that are not read",
                // Variation sequences and a Macintosh subtable come last, where the
                // search starts, and are passed over.
                cmap_of(&[
                    (3, 1, format4(&[(0x41, 0x41, d(0x41, 2), 0)], &[])),
                    (0, 5, format4(&[(0x43, 0x43, d(0x43, 1), 0)], &[])),
                    (1, 0, format4(&[(0x42, 0x42, d(0x42, 3), 0)], &[])),
                ]),
            ),
            (
                "symbol over Unicode",
                // The symbol subtable comes first, so it wins only by its kind.
                cmap_of(&[
                    (3, 0, format4(&[(0xF041, 0xF043, d(0xF041, 2), 0)], &[])),
                    (3, 10, groups_subtable(12, &[(0x100, 0x100, 1)])),
                ]),
            ),
        ];
        for (name, cmap) in cases {
            let mut book = FontBook::new();
            let font = TestFont::new("Test Map").with_table(b"cmap", cmap);
            book.add_bytes(font.build(), FontOrigin::User).unwrap();
            let face = book.load(FaceId(0)).unwrap();
            let map = face.char_map();
            let mut covered = 0;
            for code_point in (0..=0x2_0010).chain(0x10_FFF0..=0x10_FFFF) {
                let Some(ch) = char::from_u32(code_point) else {
                    continue;
                };
                // The low range of a symbol font is found through the alias only.
                if face.is_symbol() && code_point <= 0xFF {
                    continue;
                }
                let covers = !book.faces_covering(ch).is_empty();
                assert_eq!(covers, map.map(ch).is_some(), "{name}: {ch:?}");
                covered += usize::from(covers);
            }
            assert!(covered > 0, "{name} covers nothing");
        }
    }

    #[test]
    fn only_a_repeated_closing_segment_may_follow_one_at_u_ffff() {
        // A segment at U+FFFF in the middle, then one for U+0060: of the five segments, a
        // lookup of U+0060 first meets the one at U+FFFF and searches to its left, where
        // it finds nothing.
        let d = |code: u16| 1_u16.wrapping_sub(code);
        let cmap = testing::cmap_format4(&[
            (0x41, 0x41, d(0x41)),
            (0x42, 0x42, d(0x42)),
            (0xFFFF, 0xFFFF, 1),
            (0x60, 0x60, d(0x60)),
        ]);
        let mut book = FontBook::new();
        let font = TestFont::new("Test Map").with_table(b"cmap", cmap);
        book.add_bytes(font.build(), FontOrigin::User).unwrap();
        assert_eq!(book.load(FaceId(0)).unwrap().glyph_id('`'), None);
        assert!(book.faces_covering('`').is_empty());
    }

    #[test]
    fn groups_past_the_end_of_unicode_are_still_in_order() {
        // A lookup of A first meets group 2, past A, so it searches on to its right, and
        // finds nothing: the group past the end of Unicode does not end the order check.
        for format in [12, 13] {
            let cmap = testing::cmap_groups(
                format,
                &[
                    (0x41, 0x41, 2),
                    (0x11_0000, 0x11_0000, 2),
                    (0x10, 0x10, 2),
                    (0x11, 0x11, 2),
                ],
            );
            let mut book = FontBook::new();
            let font = TestFont::new("Test Map").with_table(b"cmap", cmap);
            book.add_bytes(font.build(), FontOrigin::User).unwrap();
            assert_eq!(book.load(FaceId(0)).unwrap().glyph_id('A'), None);
            assert!(book.faces_covering('A').is_empty(), "format {format}");
        }
    }

    #[test]
    fn clones_share_what_was_read() {
        let dir = TempDir::new("coverage-clone");
        let path = dir.0.join("sans.ttf");
        fs::write(&path, TestFont::new("Test Sans").build()).unwrap();
        let mut book = FontBook::new();
        book.add_dir(&dir.0).unwrap();
        let copy = book.clone();
        assert_eq!(book.faces_covering('A'), [FaceId(0)]);
        // The clone answers from what the book read, without the file.
        fs::remove_file(&path).unwrap();
        assert_eq!(copy.faces_covering('A'), [FaceId(0)]);
    }

    /// A collection of `count` faces that all point at the one font in it.
    fn one_font_many_faces(font: &[u8], count: u32) -> Vec<u8> {
        let mut ttc = Vec::new();
        ttc.extend_from_slice(b"ttcf");
        ttc.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
        ttc.extend_from_slice(&count.to_be_bytes());
        let start = 12 + 4 * count;
        for _ in 0..count {
            ttc.extend_from_slice(&start.to_be_bytes());
        }
        let mut font = font.to_vec();
        let tables = usize::from(u16::from_be_bytes([font[4], font[5]]));
        for record in 0..tables {
            let at = 12 + 16 * record + 8;
            let offset = u32::from_be_bytes(font[at..at + 4].try_into().unwrap());
            font[at..at + 4].copy_from_slice(&(offset + start).to_be_bytes());
        }
        ttc.extend_from_slice(&font);
        ttc
    }

    /// The test font with a format 13 map of every code point to glyph 1.
    fn everything(family: &str) -> Vec<u8> {
        let cmap = testing::cmap_groups(13, &[(0, 0x10_FFFF, 1)]);
        TestFont::new(family).with_table(b"cmap", cmap).build()
    }

    #[test]
    fn faces_that_share_a_map_read_it_once() {
        // 1024 faces of one font that maps all of Unicode: read once, within the budget,
        // and every face covers it.
        let mut book = FontBook::new();
        book.add_bytes(
            one_font_many_faces(&everything("Test All"), 1024),
            FontOrigin::Document,
        )
        .unwrap();
        assert_eq!(book.len(), 1024);
        assert_eq!(book.faces_covering('\u{4E00}').len(), 1024);
        assert_eq!(book.faces_covering(char::MAX).len(), 1024);
    }

    #[test]
    fn a_map_costs_its_groups_not_its_characters() {
        // Three faces with maps of their own, each one group of all of Unicode: a step
        // each, so all of them are read, however many characters they map.
        let ttc = testing::collection(&[
            everything("Test One"),
            everything("Test Two"),
            everything("Test Three"),
        ]);
        let mut book = FontBook::new();
        book.add_bytes(ttc, FontOrigin::Document).unwrap();
        book.add_bytes(everything("Test Four"), FontOrigin::Document)
            .unwrap();
        for ch in ['A', '\u{4E00}', '\u{10_FFFF}'] {
            assert_eq!(
                families(&book, &book.faces_covering(ch)),
                ["Test One", "Test Two", "Test Three", "Test Four"],
                "{ch:?}"
            );
        }
    }

    #[test]
    fn a_map_that_needs_too_many_ranges_covers_nothing() {
        // Every other code point from 0: 70,000 ranges, more than a file may keep.
        let groups: Vec<(u32, u32, u32)> = (0..70_000).map(|i| (2 * i, 2 * i, 1)).collect();
        let sparse =
            TestFont::new("Test Sparse").with_table(b"cmap", testing::cmap_groups(12, &groups));
        let mut book = FontBook::new();
        book.add_bytes(sparse.build(), FontOrigin::User).unwrap();
        let face = book.load(FaceId(0)).unwrap();
        assert!(face.glyph_id('\u{2}').is_some());
        assert!(book.faces_covering('\u{2}').is_empty());
        // Half as many fit.
        let groups: Vec<(u32, u32, u32)> = (0..30_000).map(|i| (2 * i, 2 * i, 1)).collect();
        let fewer =
            TestFont::new("Test Fewer").with_table(b"cmap", testing::cmap_groups(12, &groups));
        book.add_bytes(fewer.build(), FontOrigin::User).unwrap();
        assert_eq!(
            families(&book, &book.faces_covering('\u{2}')),
            ["Test Fewer"]
        );
    }

    #[test]
    fn recognizes_font_extensions() {
        for name in ["a.ttf", "a.OTF", "a.ttc", "a.otc"] {
            assert!(is_font_file(Path::new(name)), "{name}");
        }
        for name in ["a.fon", "a.woff2", "a", "ttf"] {
            assert!(!is_font_file(Path::new(name)), "{name}");
        }
    }
}
