//! Checks the layout's Symbol table against `data/symbol.txt`, Adobe's Symbol encoding in
//! Unicode's mapping files. This test and `data/` are left out of the published crate;
//! see THIRD-PARTY-NOTICES.md.
#![cfg(feature = "fuzzing")]

use std::collections::BTreeMap;

use officewright_layout::fuzzing::symbol_unicode;

/// The Symbol codes Adobe maps only to its Corporate Use Subarea for which the layout
/// draws a standard character: the serif and sans-serif registered, copyright and
/// trademark signs.
const OURS: [(u32, char); 6] = [
    (0xD2, '\u{00AE}'),
    (0xD3, '\u{00A9}'),
    (0xD4, '\u{2122}'),
    (0xE2, '\u{00AE}'),
    (0xE3, '\u{00A9}'),
    (0xE4, '\u{2122}'),
];

#[test]
fn every_symbol_code_reads_as_adobes_table_says() -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/data/symbol.txt"))?;
    // Each code with its Unicode values in the file's order.
    let mut table: BTreeMap<u32, Vec<char>> = BTreeMap::new();
    for line in file
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
    {
        let mut fields = line.split('\t');
        let unicode = u32::from_str_radix(fields.next().ok_or("no Unicode value")?, 16)?;
        let code = u32::from_str_radix(fields.next().ok_or("no code")?, 16)?;
        table
            .entry(code)
            .or_default()
            .push(char::from_u32(unicode).ok_or("not a character")?);
    }
    assert_eq!(table.len(), 189);
    for (code, chars) in &table {
        let standard: Vec<char> = chars
            .iter()
            .copied()
            .filter(|&ch| !('\u{E000}'..='\u{F8FF}').contains(&ch))
            .collect();
        // Of two values, the Greek letter (mu, not the micro sign; Omega, not the ohm
        // sign), else the first.
        let expected = standard
            .iter()
            .copied()
            .find(|&ch| ('\u{0370}'..='\u{03FF}').contains(&ch))
            .or_else(|| standard.first().copied())
            .or_else(|| {
                OURS.iter()
                    .find(|(ours, _)| ours == code)
                    .map(|(_, ch)| *ch)
            });
        assert_eq!(symbol_unicode("Symbol", *code), expected, "{code:02X}");
        assert_eq!(
            symbol_unicode("Symbol", 0xF000 + code),
            expected,
            "F0{code:02X}"
        );
    }
    // Codes not in the file have no character.
    for code in 0x20..=0xFF {
        if !table.contains_key(&code) {
            assert_eq!(symbol_unicode("Symbol", code), None, "{code:02X}");
        }
    }
    Ok(())
}
