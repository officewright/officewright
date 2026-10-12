/// Bounds on what reading a package may consume.
///
/// The defaults are far above what real documents need and low enough that a hostile
/// file cannot exhaust memory: across the Apache POI and LibreOffice test documents,
/// packages have at most about 1,700 entries.
///
/// ```
/// use officewright_opc::Limits;
///
/// let limits = Limits::new().with_max_part_size(64 << 20);
/// assert_eq!(limits.max_part_size(), 64 << 20);
/// assert_eq!(limits.max_entries(), Limits::new().max_entries());
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    entries: u64,
    part_size: u64,
}

impl Limits {
    /// The default limits: 65,536 ZIP entries and 256 MiB per part.
    pub const fn new() -> Self {
        Self {
            entries: 1 << 16,
            part_size: 256 << 20,
        }
    }

    /// The most entries a ZIP archive may have.
    pub const fn max_entries(&self) -> u64 {
        self.entries
    }

    /// Sets [`max_entries`](Self::max_entries).
    #[must_use]
    pub const fn with_max_entries(mut self, value: u64) -> Self {
        self.entries = value;
        self
    }

    /// The largest uncompressed size of one part, in bytes.
    pub const fn max_part_size(&self) -> u64 {
        self.part_size
    }

    /// Sets [`max_part_size`](Self::max_part_size).
    #[must_use]
    pub const fn with_max_part_size(mut self, value: u64) -> Self {
        self.part_size = value;
        self
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new()
    }
}
