//! Reads the input as a ZIP archive and decompresses every entry.
#![no_main]

use libfuzzer_sys::fuzz_target;
use officewright_opc::Limits;
use officewright_opc::zip::Archive;

fuzz_target!(|data: &[u8]| {
    // Small enough that a zip bomb shows up as a limit, not as an out-of-memory crash.
    let limits = Limits::new().with_max_part_size(1 << 20);
    if let Ok(archive) = Archive::new(data, &limits) {
        for entry in archive.entries() {
            if let Ok(bytes) = entry.read() {
                assert_eq!(bytes.len() as u64, entry.size(), "{}", entry.name());
            }
        }
    }
});
