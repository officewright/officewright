use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufReader, Cursor};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{error, fmt};

use crate::error::{Error, Result};
use crate::face::{EmbeddingPermissions, FaceProperties, FontNames, OutlineFormat};
use crate::font::FontFace;
use crate::index::{self, IndexError, IndexedFace};
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
}

/// A font file, or face of one, that a [`FontBook`] could not index.
#[derive(Debug)]
pub struct SkippedFont {
    path: Option<PathBuf>,
    error: Error,
}

impl SkippedFont {
    /// The file, `None` for a font added from memory.
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
        match (&self.path, &self.error) {
            // The error already names the file.
            (_, Error::Read { .. }) => {}
            (Some(path), _) => write!(f, "{}: ", path.display())?,
            (None, _) => f.write_str("font data: ")?,
        }
        self.error.fmt(f)?;
        let mut source = error::Error::source(&self.error);
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
/// [`skipped`](Self::skipped). A `FontBook` is `Send` and `Sync`.
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
#[derive(Debug, Default)]
pub struct FontBook {
    faces: Vec<FaceInfo>,
    skipped: Vec<SkippedFont>,
    /// Canonical paths of the files already added, so a font reachable through
    /// several directories or links is indexed once.
    files: HashSet<PathBuf>,
    /// Canonical paths of the files that could not be indexed, and the paths of font
    /// files that could not be resolved.
    failed: HashSet<PathBuf>,
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

    /// The files and faces that could not be indexed.
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
        self.add_resolved_file(path, canonical, &metadata, origin)
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
        match result {
            Ok(_) => self.files.insert(canonical),
            Err(_) => self.failed.insert(canonical),
        };
        result
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
        let mut added = 0;
        let mut errors = Vec::new();
        for (index, face) in faces {
            match face {
                Ok(face) => {
                    let keys = NameKeys::new(&face);
                    self.faces.push(FaceInfo {
                        source: source.clone(),
                        index,
                        origin,
                        properties: face.properties,
                        keys,
                    });
                    added += 1;
                }
                Err(error) => errors.push(error),
            }
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
                error,
            }));
        Ok(added)
    }

    /// Adds the fonts under `root`; unreadable files and directories are skipped.
    fn scan_dir(&mut self, root: &Path, origin: FontOrigin) -> usize {
        let mut added = 0;
        let mut visited = HashSet::new();
        let mut stack = vec![(root.to_owned(), 0)];
        while let Some((dir, depth)) = stack.pop() {
            // Following links can lead back to a directory already searched.
            let Ok(canonical_dir) = fs::canonicalize(&dir) else {
                continue;
            };
            if !visited.insert(canonical_dir.clone()) {
                continue;
            }
            let entries = match fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(source) => {
                    self.skipped.push(SkippedFont {
                        path: Some(dir.clone()),
                        error: Error::Read { path: dir, source },
                    });
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
                // `metadata` follows links, so linked fonts and directories count.
                let metadata = fs::metadata(&path);
                if metadata.as_ref().is_ok_and(fs::Metadata::is_dir) {
                    if depth < MAX_DEPTH {
                        subdirs.push((path, depth + 1));
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
                    // file, is listed, once: it is remembered under its canonical
                    // directory, however the directory was reached.
                    Err(source) => {
                        let key = path
                            .file_name()
                            .map_or_else(|| path.clone(), |name| canonical_dir.join(name));
                        if self.failed.insert(key) {
                            self.skipped.push(SkippedFont {
                                path: Some(path.clone()),
                                error: Error::Read { path, source },
                            });
                        }
                        continue;
                    }
                };
                // Sockets, FIFOs and devices are not fonts, whatever their names.
                if !metadata.is_file() {
                    continue;
                }
                // A file that failed before is not read and reported again.
                if self.failed.contains(&canonical) {
                    continue;
                }
                match self.add_resolved_file(&path, canonical, &metadata, origin) {
                    Ok(count) => added += count,
                    Err(error) => self.skipped.push(SkippedFont {
                        path: Some(path),
                        error,
                    }),
                }
            }
            stack.extend(subdirs.into_iter().rev());
        }
        added
    }
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
            let _ = fs::remove_dir_all(&self.0);
        }
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
    #[cfg(unix)]
    fn survives_directory_link_loops() {
        let dir = TempDir::new("loop");
        fs::write(dir.0.join("sans.ttf"), TestFont::new("Test Sans").build()).unwrap();
        std::os::unix::fs::symlink(&dir.0, dir.0.join("self")).unwrap();
        let mut book = FontBook::new();
        assert_eq!(book.add_dir(&dir.0).unwrap(), 1);
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
            error: err,
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
            error: Error::MalformedFace {
                face: 64,
                source: crate::error::Cause::new("faces 64 to 1023 were not read"),
            },
        };
        assert_eq!(
            budget.to_string(),
            "font data: font face 64: the table directory is damaged: faces 64 to 1023 were \
             not read"
        );
        let read = SkippedFont {
            path: Some(PathBuf::from("a.ttf")),
            error: Error::Read {
                path: PathBuf::from("a.ttf"),
                source: io::Error::other("disk gone"),
            },
        };
        assert_eq!(read.to_string(), "cannot read a.ttf: disk gone");
    }

    #[test]
    #[cfg(unix)]
    fn adds_fonts_through_links() {
        let dir = TempDir::new("links");
        let fonts = dir.0.join("fonts");
        let links = dir.0.join("links");
        fs::create_dir_all(&fonts).unwrap();
        fs::create_dir_all(&links).unwrap();
        let font = fonts.join("sans.ttf");
        let link = links.join("link.ttf");
        fs::write(&font, TestFont::new("Test Sans").build()).unwrap();
        std::os::unix::fs::symlink(&font, &link).unwrap();

        let mut book = FontBook::new();
        assert_eq!(book.add_file(&link).unwrap(), 1);
        assert_eq!(book.faces().next().unwrap().1.path(), Some(link.as_path()));
        // The link and its target are the same file.
        assert_eq!(book.add_file(&font).unwrap(), 0);

        let mut book = FontBook::new();
        assert_eq!(book.add_dir(&links).unwrap(), 1);
        assert!(book.skipped().is_empty(), "{:?}", book.skipped());
    }

    #[test]
    #[cfg(unix)]
    fn lists_font_files_that_cannot_be_resolved_once() {
        let dir = TempDir::new("dangling");
        let fonts = dir.0.join("fonts");
        fs::create_dir(&fonts).unwrap();
        let link = fonts.join("gone.ttf");
        std::os::unix::fs::symlink(fonts.join("removed.ttf"), &link).unwrap();
        // A dangling link with another name is not a font file and is not listed.
        std::os::unix::fs::symlink(fonts.join("removed.txt"), fonts.join("gone.txt")).unwrap();
        // The same directory reached through a link to it.
        let alias = dir.0.join("alias");
        std::os::unix::fs::symlink(&fonts, &alias).unwrap();
        let mut book = FontBook::new();
        book.add_dir(&fonts).unwrap();
        book.add_dir(&fonts).unwrap();
        book.add_dir(&alias).unwrap();
        assert_eq!(book.skipped().len(), 1, "{:?}", book.skipped());
        assert_eq!(book.skipped()[0].path(), Some(link.as_path()));
        assert!(matches!(book.skipped()[0].error(), Error::Read { .. }));
    }

    #[test]
    fn load_reports_ids_from_another_book() {
        let book = FontBook::new();
        assert!(matches!(
            book.load(FaceId(3)),
            Err(Error::FaceIndexOutOfRange { index: 3, count: 0 })
        ));
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
