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
    let mins: Vec<f64> = columns.iter().map(|column| column.min).collect();
    if columns.iter().any(|column| column.percent.is_some()) {
        // The shares stand in for what the columns want, fitted to the table's width as
        // any other wants are: a column raised to its minimum takes it from the others,
        // and minima that do not fit are cut, or kept with `grow`.
        let (shares, width) = by_percent(columns, target);
        return fit(&mins, &shares, Target::Exact(width), grow);
    }
    let wanted: Vec<f64> = columns.iter().map(Column::wanted).collect();
    fit(&mins, &wanted, target, grow)
}

/// The narrowest and the widest the columns are laid out together when they may take as
/// much room as they want: their minima, and what [`widths`] gives them in `room`, as
/// wide as a table may be.
pub(crate) fn natural(columns: &[Column], room: f64) -> (f64, f64) {
    let min: f64 = columns.iter().map(|column| column.min).sum();
    let max: f64 = widths(columns, Target::Auto(room), false).iter().sum();
    (min, max.max(min))
}

/// Widths of columns that want `wanted`, never less than `mins`, fitted to `target`.
fn fit(mins: &[f64], wanted: &[f64], target: Target, grow: bool) -> Vec<f64> {
    let wanted: Vec<f64> = wanted.iter().zip(mins).map(|(w, m)| w.max(*m)).collect();
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
    if grow {
        mins.to_vec()
    } else {
        scaled(mins, room)
    }
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
/// each wants, at most the width it may take. Returns what each column then wants, and the
/// table's width.
fn by_percent(columns: &[Column], target: Target) -> (Vec<f64>, f64) {
    let mut left = 100.0_f64;
    let shares: Vec<Option<f64>> = columns
        .iter()
        .map(|column| {
            column.percent.map(|percent| {
                // A share that is no finite number takes nothing, so that what is left
                // stays one.
                let share = if percent.is_finite() {
                    percent.clamp(0.0, left)
                } else {
                    0.0
                };
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
    let wanted = columns
        .iter()
        .zip(&shares)
        .map(|(column, share)| match share {
            Some(share) => share * width,
            None if rest > 0.0 => column.wanted() * spare / rest,
            None => 0.0,
        })
        .collect();
    (wanted, width)
}

/// What the columns of a table need, as cells that span several of them add to it
/// ([`Needs::spread`]), with the sums of their minima, maxima and wants over any range of
/// them at hand: each spanning cell takes time in the logarithm of the columns, however many
/// it spans, so that a table of many rows of wide spans takes time in proportion to its
/// cells.
pub(crate) struct Needs {
    columns: Vec<Column>,
    min: Sums,
    max: Sums,
    wanted: Sums,
}

impl Needs {
    /// The needs of `columns`, as the cells that lie in one column alone set them.
    pub(crate) fn new(columns: Vec<Column>) -> Self {
        let sums = |value: fn(&Column) -> f64| Sums::of(columns.iter().map(value));
        Self {
            min: sums(|column| column.min),
            max: sums(|column| column.max),
            wanted: sums(Column::wanted),
            columns,
        }
    }

    /// Adds what a cell over the columns `range` needs beyond what they have: its minimum,
    /// its maximum and its preferred width each go to the last of them (Word's render of
    /// case 8: a span of 300 pt over cells of 50 and 100 pt makes columns of 50 and 250; a
    /// hypothesis among the rules that fit it, until a Word render of a span over three
    /// columns). A spanning cell's preferred width in percent is not used, a hypothesis
    /// too. A range past the columns adds nothing.
    pub(crate) fn spread(&mut self, range: std::ops::Range<usize>, cell: Column) {
        if range.is_empty() || range.end > self.columns.len() {
            return;
        }
        let has_min = self.min.range(range.clone());
        let has_max = self.max.range(range.clone());
        let has_wanted = self.wanted.range(range.clone());
        let last = range.end - 1;
        let Some(column) = self.columns.get_mut(last) else {
            return;
        };
        let before = *column;
        column.min += (cell.min - has_min).max(0.0);
        column.max += (cell.max - has_max).max(0.0);
        if let Some(preferred) = cell.preferred {
            let more = preferred - has_wanted;
            if more > 0.0 {
                // What the column wanted before the span raised its minimum: the span's
                // minimum and its preferred width are needs of one cell, not one on top of
                // the other.
                column.preferred = Some(before.wanted() + more);
            }
        }
        let after = *column;
        self.min.add(last, after.min - before.min);
        self.max.add(last, after.max - before.max);
        self.wanted.add(last, after.wanted() - before.wanted());
    }

    /// The columns' needs.
    pub(crate) fn into_columns(self) -> Vec<Column> {
        self.columns
    }
}

/// Sums of values over ranges, which change one at a time (a Fenwick tree).
struct Sums(Vec<f64>);

impl Sums {
    fn of(values: impl ExactSizeIterator<Item = f64>) -> Self {
        let mut sums = Self(vec![0.0; values.len()]);
        for (index, value) in values.enumerate() {
            sums.add(index, value);
        }
        sums
    }

    /// Adds `delta` to value `index`.
    fn add(&mut self, index: usize, delta: f64) {
        let mut at = index + 1;
        while let Some(sum) = self.0.get_mut(at - 1) {
            *sum += delta;
            at += at & at.wrapping_neg();
        }
    }

    /// The sum of the first `end` values.
    fn prefix(&self, end: usize) -> f64 {
        let mut total = 0.0;
        let mut at = end.min(self.0.len());
        while at > 0 {
            total += self.0.get(at - 1).copied().unwrap_or(0.0);
            at -= at & at.wrapping_neg();
        }
        total
    }

    /// The sum of the values in `range`.
    fn range(&self, range: std::ops::Range<usize>) -> f64 {
        self.prefix(range.end) - self.prefix(range.start)
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
    fn shares_raised_to_their_minima_take_the_room_from_the_others() {
        // Review of #415: a 100 % table of three 33 % cells, one with a word of 255.75 pt
        // with its margins: the table stays 450.8 pt wide, the other columns giving way.
        let share = |min: f64| Column {
            min,
            max: min,
            preferred: None,
            percent: Some(33.0),
        };
        let columns = [share(255.75), share(22.99), share(22.99)];
        let fitted = widths(&columns, Target::Exact(450.8), false);
        assert!(
            (fitted.iter().sum::<f64>() - 450.8).abs() < 1e-9,
            "{fitted:?}"
        );
        assert!(fitted[0] >= 255.75 && fitted[1] >= 22.99, "{fitted:?}");
        // Minima past the width are cut to it, or with `growAutofit` kept (case 11).
        let long = [share(249.75), share(249.75), share(249.75)];
        let cut = widths(&long, Target::Exact(450.8), false);
        assert!(close(&cut, &[150.27; 3]), "{cut:?}");
        assert_eq!(widths(&long, Target::Exact(450.8), true), [249.75; 3]);
        // A column without a share takes what is left, in proportion to what it wants.
        let mixed = [
            Column {
                min: 10.0,
                max: 10.0,
                preferred: None,
                percent: Some(50.0),
            },
            Column {
                min: 10.0,
                max: 30.0,
                ..Column::default()
            },
        ];
        assert_eq!(widths(&mixed, Target::Exact(200.0), false), [100.0, 100.0]);
        // As wide as the shares need, laid out with room to spare: 30 / 0.5.
        assert_eq!(natural(&mixed, 1000.0), (20.0, 60.0));
        let (min, max) = natural(&word(), 1000.0);
        assert!(close(&[min, max], &[107.98, 263.0]), "{min} {max}");
    }

    #[test]
    fn a_spanning_cell_gives_what_its_columns_lack_to_the_last_of_them() {
        let spread = |columns: Vec<Column>, spans: &[(std::ops::Range<usize>, Column)]| {
            let mut needs = Needs::new(columns);
            for (range, cell) in spans {
                needs.spread(range.clone(), *cell);
            }
            needs.into_columns()
        };
        // Case 8: a span of 300 pt over cells of 50 and 100 pt.
        let preferred = |width| Column {
            preferred: Some(width),
            ..Column::default()
        };
        let columns = spread(
            vec![preferred(50.0), preferred(100.0)],
            &[(0..2, preferred(300.0))],
        );
        assert_eq!(widths(&columns, Target::Auto(450.8), false), [50.0, 250.0]);
        // Review of #415: a span with both a minimum and a preferred width wider than its
        // columns' is one cell's needs, the preferred width counted from what the last
        // column wanted before the span raised its minimum.
        let columns = spread(
            vec![
                Column {
                    min: 10.0,
                    max: 10.0,
                    preferred: Some(50.0),
                    percent: None,
                },
                Column {
                    min: 5.0,
                    max: 5.0,
                    preferred: Some(100.0),
                    percent: None,
                },
            ],
            &[(
                0..2,
                Column {
                    min: 200.0,
                    preferred: Some(300.0),
                    ..Column::default()
                },
            )],
        );
        assert_eq!(widths(&columns, Target::Auto(450.8), false), [50.0, 250.0]);
        // A span's word wider than its columns' widest makes the last one wider.
        let narrow = Column {
            min: 20.0,
            max: 30.0,
            ..Column::default()
        };
        let wide = Column {
            min: 100.0,
            max: 100.0,
            ..Column::default()
        };
        let columns = spread(vec![narrow; 2], &[(0..2, wide)]);
        assert_eq!((columns[1].min, columns[1].max), (80.0, 70.0));
        // A range past the columns, or empty, changes nothing.
        let columns = spread(vec![narrow; 2], &[(1..5, wide), (1..1, wide)]);
        assert_eq!(columns, vec![narrow; 2]);
    }

    #[test]
    fn spans_add_up_as_their_columns_change_one_after_another() {
        // Overlapping spans, each seeing what the ones before it gave: the sums over ranges
        // agree with adding the columns up one by one.
        let naive = |columns: &mut Vec<Column>, range: std::ops::Range<usize>, cell: Column| {
            let has = |f: fn(&Column) -> f64, columns: &[Column]| -> f64 {
                columns[range.clone()].iter().map(f).sum()
            };
            let (min, max, wanted) = (
                has(|c| c.min, columns),
                has(|c| c.max, columns),
                has(Column::wanted, columns),
            );
            let last = &mut columns[range.end - 1];
            let before = *last;
            last.min += (cell.min - min).max(0.0);
            last.max += (cell.max - max).max(0.0);
            if let Some(preferred) = cell.preferred {
                if preferred - wanted > 0.0 {
                    last.preferred = Some(before.wanted() + preferred - wanted);
                }
            }
        };
        let start: Vec<Column> = (0..12)
            .map(|i| Column {
                min: f64::from(i % 4) * 3.0,
                max: f64::from(i % 5) * 7.0 + 1.0,
                preferred: (i % 3 == 0).then_some(f64::from(i) * 4.0),
                percent: None,
            })
            .collect();
        let spans: Vec<(std::ops::Range<usize>, Column)> = (0..30)
            .map(|i: usize| {
                let start = (i * 7) % 11;
                let end = (start + 1 + (i * 5) % 6).min(12);
                let width = (i * 13 % 40) as f64;
                (
                    start..end,
                    Column {
                        min: width,
                        max: width * 2.0,
                        preferred: (i % 2 == 0).then_some(width * 3.0),
                        percent: None,
                    },
                )
            })
            .collect();
        let mut expected = start.clone();
        let mut needs = Needs::new(start);
        for (range, cell) in spans {
            naive(&mut expected, range.clone(), cell);
            needs.spread(range, cell);
        }
        let columns = needs.into_columns();
        for (a, b) in columns.iter().zip(&expected) {
            assert!(
                (a.min - b.min).abs() < 1e-9 && (a.max - b.max).abs() < 1e-9,
                "{a:?} {b:?}"
            );
            assert!((a.wanted() - b.wanted()).abs() < 1e-9, "{a:?} {b:?}");
        }
    }

    #[test]
    fn many_wide_spans_take_time_in_proportion_to_them() {
        // Review of #415: 20,000 rows each with a cell over all 20,000 columns. Adding up
        // the columns of each span one by one takes 400 million steps; the sums over ranges
        // take a few for each.
        let count = 20_000;
        let start = std::time::Instant::now();
        let mut needs = Needs::new(vec![Column::default(); count]);
        for i in 0..count {
            needs.spread(
                0..count,
                Column {
                    min: i as f64,
                    max: i as f64,
                    ..Column::default()
                },
            );
        }
        let columns = needs.into_columns();
        assert_eq!(columns[count - 1].min, (count - 1) as f64);
        assert!(start.elapsed().as_secs_f64() < 2.0, "{:?}", start.elapsed());
    }

    #[test]
    fn a_share_that_is_no_number_takes_nothing() {
        let column = |percent| Column {
            min: 10.0,
            max: 10.0,
            preferred: None,
            percent: Some(percent),
        };
        // The column of no share wants only its minimum, the other its 50 %: the table of
        // 100 pt shares itself between them as 10 to 50.
        let shares = widths(
            &[column(f64::NAN), column(50.0)],
            Target::Exact(100.0),
            false,
        );
        assert!(close(&shares, &[100.0 / 6.0, 500.0 / 6.0]), "{shares:?}");
    }
}
