//! Page layout engine: text flow, line breaking and tables.
//!
//! The engine lays out documents given in its own intermediate representation, the
//! layout IR of the `ir` module, which the format crates build from their documents:
//! DOCX first, later XLSX and PPTX. Its rules are Word's, since Word is what users
//! compare the output with.
//!
//! The IR is public, so producers in other crates can build it, but it is exempt from
//! semantic versioning until officewright 1.0. Its documentation is hidden unless the
//! feature `unstable-ir` is on.
//!
//! What a layout run does differently from Word, such as a font it substitutes or a
//! value it brings within Word's limits, goes into a [`LayoutReport`].
//!
//! ```
//! use officewright_core::{PageSize, Pt};
//! use officewright_layout::ir::{Document, Inline, PageSetup, Paragraph, RunStyle, Section};
//!
//! let mut document = Document::default();
//! let text = document.add_run_style(RunStyle::default().with_size(Pt(11.0)));
//! let paragraph = Paragraph::new(text).with_inlines([Inline::text("Hello, world", text)]);
//! document.add_section(Section::new(PageSetup::new(PageSize::A4)).with_blocks([paragraph.into()]));
//! document.validate()?;
//! let report = document.clamp();
//! assert!(report.is_empty());
//! # Ok::<(), officewright_layout::Error>(())
//! ```
#![forbid(unsafe_code)]

mod error;
#[cfg(feature = "fuzzing")]
pub mod fuzzing;
#[cfg_attr(not(feature = "unstable-ir"), doc(hidden))]
pub mod ir;
mod report;
mod shape;

pub use error::{Error, Result};
pub use report::{LayoutReport, Location, ReportEntry, ReportKind};
