//! Reads the input as a ZIP archive and decompresses every entry, then opens it as an
//! OOXML package and reads every part.
#![no_main]

use libfuzzer_sys::fuzz_target;
use officewright_opc::zip::Archive;
use officewright_opc::{Limits, Package};

fuzz_target!(|data: &[u8]| {
    // Small enough that a zip bomb shows up as a limit, not as an out-of-memory crash.
    let limits = Limits::new()
        .with_max_part_size(1 << 20)
        .with_max_total_size(4 << 20);
    if let Ok(archive) = Archive::new(data, &limits) {
        for entry in archive.entries() {
            if let Ok(bytes) = entry.read() {
                assert_eq!(bytes.len() as u64, entry.size(), "{}", entry.name());
            }
        }
    }
    if let Ok(package) = Package::from_bytes(data, &limits) {
        for part in package.parts() {
            assert_eq!(
                package.part(part.name()).map(|p| p.name()),
                Some(part.name())
            );
            let _ = (part.content_type(), part.read());
            for relationship in part.relationships() {
                if let Some(target) = relationship.target_part() {
                    assert!(target.as_str().starts_with('/'));
                    assert!(!target.as_str().split('/').any(|s| s == ".."));
                }
            }
        }
    }
});
