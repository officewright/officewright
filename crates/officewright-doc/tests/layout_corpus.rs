//! Every Word document of the committed corpus that opens translates into layout IR that
//! `validate` accepts. It reads `tests/corpus` at the root of the repository, so it is
//! left out of the published crate.
#![cfg(feature = "pdf")]

use std::path::{Path, PathBuf};

use officewright_doc::Document;

#[test]
fn every_committed_document_translates_into_valid_ir() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus");
    let mut pending = vec![root];
    let mut files: Vec<PathBuf> = Vec::new();
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| matches!(extension, "docx" | "docm" | "dotx" | "dotm"))
            {
                files.push(path);
            }
        }
    }
    files.sort();
    let mut translated = 0;
    for file in &files {
        // A few corpus files are refused on purpose (tools/corpus-runner/expected-errors.tsv).
        let Ok(document) = Document::open(file) else {
            continue;
        };
        let translation = document.to_layout_ir();
        if let Err(err) = translation.document().validate() {
            panic!("{}: {err}", file.display());
        }
        translated += 1;
    }
    assert!(
        translated > 150,
        "only {translated} of {} files",
        files.len()
    );
}
