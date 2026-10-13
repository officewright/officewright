//! Which characters a face maps to a glyph.

/// The characters a face's character map maps to a glyph, as sorted ranges of code
/// points that neither overlap nor touch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Coverage(Vec<(u32, u32)>);

impl Coverage {
    /// The coverage of a list of code points, in any order and with repeats.
    pub(crate) fn from_code_points(code_points: impl IntoIterator<Item = u32>) -> Self {
        // Character maps list their code points in order, so the runs are long.
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for code_point in code_points {
            match runs.last_mut() {
                Some((start, end)) if (*start..=end.saturating_add(1)).contains(&code_point) => {
                    *end = (*end).max(code_point);
                }
                _ => runs.push((code_point, code_point)),
            }
        }
        // A damaged map may list them in any order.
        runs.sort_unstable();
        let mut ranges: Vec<(u32, u32)> = Vec::with_capacity(runs.len());
        for (start, end) in runs {
            match ranges.last_mut() {
                Some((_, last)) if start <= last.saturating_add(1) => *last = (*last).max(end),
                _ => ranges.push((start, end)),
            }
        }
        Self(ranges)
    }

    /// Whether the face maps `ch`.
    pub(crate) fn contains(&self, ch: char) -> bool {
        let code_point = u32::from(ch);
        // The ranges that start at or before the code point; the last of them is the one
        // that can hold it.
        let before = self.0.partition_point(|&(start, _)| start <= code_point);
        before
            .checked_sub(1)
            .and_then(|last| self.0.get(last))
            .is_some_and(|&(_, end)| code_point <= end)
    }

    /// The number of ranges.
    pub(crate) fn range_count(&self) -> usize {
        self.0.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_runs_in_any_order() {
        let coverage = Coverage::from_code_points([0x42, 0x41, 0x43, 0x41, 0x100, 0x45, 0x44]);
        assert_eq!(coverage.0, [(0x41, 0x45), (0x100, 0x100)]);
        for ch in ['A', 'B', 'E', '\u{100}'] {
            assert!(coverage.contains(ch), "{ch:?}");
        }
        for ch in ['@', 'F', '\u{FF}', '\u{101}', '\0', char::MAX] {
            assert!(!coverage.contains(ch), "{ch:?}");
        }
    }

    #[test]
    fn covers_the_ends_of_unicode() {
        let coverage = Coverage::from_code_points([0, 0x10_FFFF, 0x10_FFFE]);
        assert_eq!(coverage.0, [(0, 0), (0x10_FFFE, 0x10_FFFF)]);
        assert!(coverage.contains('\0'));
        assert!(coverage.contains(char::MAX));
        assert!(!Coverage::default().contains('A'));
        assert_eq!(Coverage::default().range_count(), 0);
    }
}
