//! Fitting a table's columns to their content (design 8.2), by the rules Word's renders of
//! `E-table-autofit-15`, `-14`, `-constrained-14` and `-grow-14` show to 0.05 pt.
//!
//! Each column has a minimum, the widest word of its cells with their margins, and a
//! maximum, the widest of their paragraphs laid out on one line with their margins; a
//! cell's preferred width in points stands in for its column's maximum, raised to the
//! minimum. Then:
//! - when the maxima fit the width the table may take, each column gets its maximum, and
//!   the table is no wider (case 1);
//! - a table of an explicit width wider than its maxima scales every maximum by the same
//!   factor (case 2, 100 %);
//! - between the minima and the maxima, each column gets `min + (max − min) · (W − Σmin) /
//!   (Σmax − Σmin)` (case 4: neither a proportional scale nor an equal split);
//! - below the minima, the columns are cut to the width in proportion to their minima, and
//!   the words in them broken between characters (case 11, whose minima are equal, so an
//!   equal split fits it too: a hypothesis until a Word render settles it); with
//!   `growAutofit` in Word 2010's mode and earlier, they keep their minima and the table
//!   runs past the margin.
//!
//! Percentages are taken from the first column on until they make 100 %, and the table is
//! as wide as its widest column's share needs, at most the width it may take (case 5).

/// What a column needs, from the cells that lie in it alone, and from those that span it
/// with others (see [`spread`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Column {
    /// The widest word of its cells, with their margins.
    pub(crate) min: f64,
    /// The widest paragraph of its cells on one line, with their margins.
    pub(crate) max: f64,
    /// The widest preferred width in points of its cells, if one has one.
    pub(crate) preferred: Option<f64>,
    /// The largest preferred width in percent of its cells, if one has one.
    pub(crate) percent: Option<f64>,
}

impl Column {
    /// The width it would take with room to spare: its preferred width when it has one,
    /// else its maximum, and never less than its minimum.
    pub(crate) fn wanted(&self) -> f64 {
        self.preferred.unwrap_or(self.max).max(self.min)
    }
}

/// The width a table fits its columns to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Target {
    /// As wide as its columns want, at most this wide: the width it may take.
    Auto(f64),
    /// This wide, from a width in points or in percent.
    Exact(f64),
}

/// The widths of `columns` fitted to `target`. `grow` keeps the columns at their minima
/// when those do not fit, instead of cutting them to the width (`growAutofit`).
pub(crate) fn widths(columns: &[Column], target: Target, grow: bool) -> Vec<f64> {
    if columns.iter().any(|column| column.percent.is_some()) {
        return by_percent(columns, target);
    }
    let wanted: Vec<f64> = columns.iter().map(Column::wanted).collect();
    let mins: Vec<f64> = columns.iter().map(|column| column.min).collect();
    let (sum_min, sum_wanted) = (mins.iter().sum::<f64>(), wanted.iter().sum::<f64>());
    let room = match target {
        Target::Auto(room) => {
            if sum_wanted <= room {
                return wanted;
            }
            room
        }
        Target::Exact(width) => {
            if sum_wanted <= width {
                return scaled(&wanted, width);
            }
            width
        }
    };
    if sum_min <= room {
        let spare = sum_wanted - sum_min;
        let share = if spare > 0.0 {
            (room - sum_min) / spare
        } else {
            0.0
        };
        return mins
            .iter()
            .zip(&wanted)
            .map(|(min, wanted)| min + (wanted - min) * share)
            .collect();
    }
    if grow { mins } else { scaled(&mins, room) }
}

/// `widths` scaled to make `total` together; shared equally when they make nothing.
fn scaled(widths: &[f64], total: f64) -> Vec<f64> {
    let sum: f64 = widths.iter().sum();
    if sum > 0.0 {
        widths.iter().map(|width| width * total / sum).collect()
    } else if widths.is_empty() {
        Vec::new()
    } else {
        vec![total / widths.len() as f64; widths.len()]
    }
}

/// The widths of `columns`, some of which have a preferred width in percent, fitted to
/// `target`. Each such column takes its share of the table, from the first column on until
/// the shares make 100 % (Word's render of case 5: 40 % three times makes 40, 40 and 20);
/// the others share what is left in proportion to what they want (a hypothesis, as no
/// render mixes them). An auto table is as wide as its columns' shares need to hold what
/// each wants, at most the width it may take; every column is then raised to its minimum.
fn by_percent(columns: &[Column], target: Target) -> Vec<f64> {
    let mut left = 100.0_f64;
    let shares: Vec<Option<f64>> = columns
        .iter()
        .map(|column| {
            column.percent.map(|percent| {
                let share = percent.clamp(0.0, left);
                left -= share;
                share / 100.0
            })
        })
        .collect();
    let taken: f64 = shares.iter().flatten().sum();
    let rest: f64 = columns
        .iter()
        .zip(&shares)
        .filter(|(_, share)| share.is_none())
        .map(|(column, _)| column.wanted())
        .sum();
    let width = match target {
        Target::Exact(width) => width,
        Target::Auto(room) => {
            let by_shares = columns
                .iter()
                .zip(&shares)
                .filter_map(|(column, share)| {
                    share
                        .filter(|&share| share > 0.0)
                        .map(|share| column.wanted() / share)
                })
                .fold(0.0, f64::max);
            let by_rest = if taken < 1.0 {
                rest / (1.0 - taken)
            } else {
                0.0
            };
            by_shares.max(by_rest).min(room)
        }
    };
    let spare = (width * (1.0 - taken)).max(0.0);
    columns
        .iter()
        .zip(&shares)
        .map(|(column, share)| {
            let width = match share {
                Some(share) => share * width,
                None if rest > 0.0 => column.wanted() * spare / rest,
                None => 0.0,
            };
            width.max(column.min)
        })
        .collect()
}

/// Adds to `columns` what a cell over the columns `range` needs beyond what they have:
/// its minimum, its maximum and its preferred width each go to the last of them (Word's
/// render of case 8: a span of 300 pt over cells of 50 and 100 pt makes columns of 50 and
/// 250; a hypothesis among the rules that fit it, until a Word render of a span over three
/// columns).
pub(crate) fn spread(columns: &mut [Column], range: std::ops::Range<usize>, cell: Column) {
    let Some(spanned) = columns.get(range.clone()) else {
        return;
    };
    let has: Column = spanned
        .iter()
        .fold(Column::default(), |sum, column| Column {
            min: sum.min + column.min,
            max: sum.max + column.max,
            preferred: Some(sum.preferred.unwrap_or(0.0) + column.wanted()),
            percent: None,
        });
    let Some(last) = range
        .end
        .checked_sub(1)
        .and_then(|last| columns.get_mut(last))
    else {
        return;
    };
    last.min += (cell.min - has.min).max(0.0);
    last.max += (cell.max - has.max).max(0.0);
    if let Some(preferred) = cell.preferred {
        let more = preferred - has.preferred.unwrap_or(0.0);
        if more > 0.0 {
            last.preferred = Some(last.wanted() + more);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The columns of Word's `E-table-autofit-*` documents: "Ab rN", "A longer cell of
    /// several words rN" and "Middle text rN" with cell margins of 5.4 pt.
    fn word() -> Vec<Column> {
        [(22.99, 34.85), (42.64, 153.85), (42.35, 74.3)]
            .into_iter()
            .map(|(min, max)| Column {
                min,
                max,
                ..Column::default()
            })
            .collect()
    }

    fn close(widths: &[f64], expected: &[f64]) -> bool {
        widths.len() == expected.len()
            && widths
                .iter()
                .zip(expected)
                .all(|(a, b)| (a - b).abs() < 0.05)
    }

    #[test]
    fn columns_take_their_maxima_when_they_fit_and_share_the_width_between_minima_and_maxima() {
        // Case 1: the maxima fit 450.8 pt (Word 2013's mode), and the table is no wider.
        let auto = widths(&word(), Target::Auto(450.8), false);
        assert!(close(&auto, &[34.85, 153.85, 74.3]), "{auto:?}");
        // Case 2: 100 % scales every maximum alike, in both modes.
        let full = widths(&word(), Target::Exact(450.8), false);
        assert!(close(&full, &[59.75, 263.7, 127.35]), "{full:?}");
        let full = widths(&word(), Target::Exact(462.1), false);
        assert!(close(&full, &[61.25, 270.3, 130.55]), "{full:?}");
        // Case 4: preferred widths of 200 pt stand in for the maxima, and the width left
        // over the minima is shared in proportion to what each wants above its minimum.
        let preferred: Vec<Column> = word()
            .into_iter()
            .map(|column| Column {
                preferred: Some(200.0),
                ..column
            })
            .collect();
        let shared = widths(&preferred, Target::Auto(450.8), false);
        assert!(close(&shared, &[146.3, 152.3, 152.2]), "{shared:?}");
        let shared = widths(&preferred, Target::Auto(462.1), false);
        assert!(close(&shared, &[150.4, 155.9, 155.8]), "{shared:?}");
        // Case 3: preferred widths that fit are kept, below the maxima.
        let small: Vec<Column> = word()
            .into_iter()
            .map(|column| Column {
                preferred: Some(72.0),
                ..column
            })
            .collect();
        assert_eq!(widths(&small, Target::Auto(450.8), false), [72.0; 3]);
        // Mode 14 case 12, without preferred widths, in the 237.17 pt beside a picture.
        let beside = widths(&word(), Target::Auto(237.17), false);
        assert!(close(&beside, &[32.87, 135.32, 68.98]), "{beside:?}");
    }

    #[test]
    fn a_preferred_width_below_the_minimum_and_minima_that_do_not_fit() {
        // Case 6: a 36 pt preference in a column whose word is 249.75 pt with its margins.
        let columns = [
            Column {
                min: 249.75,
                max: 249.75,
                preferred: Some(36.0),
                percent: None,
            },
            Column {
                min: 35.0,
                max: 35.0,
                ..Column::default()
            },
            Column {
                min: 36.9,
                max: 36.9,
                ..Column::default()
            },
        ];
        assert_eq!(
            widths(&columns, Target::Auto(450.8), false),
            [249.75, 35.0, 36.9]
        );
        // Case 11: three such words do not fit, and are cut to the width; with
        // `growAutofit` they keep their minima.
        let long = [Column {
            min: 249.75,
            max: 249.75,
            ..Column::default()
        }; 3];
        let cut = widths(&long, Target::Auto(450.8), false);
        assert!(close(&cut, &[150.27, 150.27, 150.27]), "{cut:?}");
        assert_eq!(widths(&long, Target::Auto(462.1), true), [249.75; 3]);
        // Unequal minima are cut in proportion to them: a hypothesis, which equal minima
        // cannot tell from an equal split.
        let unequal = [100.0, 300.0].map(|min| Column {
            min,
            max: min,
            ..Column::default()
        });
        assert_eq!(widths(&unequal, Target::Auto(200.0), false), [50.0, 150.0]);
        assert!(close(
            &widths(&long, Target::Exact(462.1), false),
            &[154.03; 3]
        ));
    }

    #[test]
    fn percentages_are_taken_until_they_make_the_whole_and_size_the_table() {
        // Case 5: 40 % three times makes 40, 40 and 20 % of a table 153.85 / 0.4 wide.
        let columns: Vec<Column> = word()
            .into_iter()
            .map(|column| Column {
                percent: Some(40.0),
                ..column
            })
            .collect();
        let shares = widths(&columns, Target::Auto(450.8), false);
        assert!(close(&shares, &[153.85, 153.85, 76.93]), "{shares:?}");
        // The table is at most as wide as it may be.
        let narrow = widths(&columns, Target::Auto(300.0), false);
        assert!(close(&narrow, &[120.0, 120.0, 60.0]), "{narrow:?}");
    }

    #[test]
    fn a_spanning_cell_gives_what_its_columns_lack_to_the_last_of_them() {
        // Case 8: a span of 300 pt over cells of 50 and 100 pt.
        let mut columns = [
            Column {
                preferred: Some(50.0),
                ..Column::default()
            },
            Column {
                preferred: Some(100.0),
                ..Column::default()
            },
        ];
        spread(
            &mut columns,
            0..2,
            Column {
                preferred: Some(300.0),
                ..Column::default()
            },
        );
        assert_eq!(widths(&columns, Target::Auto(450.8), false), [50.0, 250.0]);
        // A span's word wider than its columns' widest makes the last one wider.
        let mut columns = [Column {
            min: 20.0,
            max: 30.0,
            ..Column::default()
        }; 2];
        spread(
            &mut columns,
            0..2,
            Column {
                min: 100.0,
                max: 100.0,
                ..Column::default()
            },
        );
        assert_eq!(columns[1].min, 80.0);
        assert_eq!(columns[1].max, 70.0);
        // A range past the columns changes nothing.
        let before = columns;
        spread(&mut columns, 1..5, Column::default());
        assert_eq!(columns, before);
    }
}
