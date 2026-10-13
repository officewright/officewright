//! A paragraph's tab stops, and the stop a tab takes the text to.

// reason: the layout itself uses this once pagination (plan PR 8) lays out pages; until
// then only the tests and the `fuzzing` module do.
#![cfg_attr(not(any(test, feature = "fuzzing")), allow(dead_code))]

use officewright_core::Pt;

use crate::ir::{CompatFlag, CompatMode, DocumentSettings, ParagraphProps, TabKind, TabLeader};

/// Where a tab takes the text, and how the text aligns there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Stop {
    /// From the start edge of the column.
    pub(crate) position: Pt,
    pub(crate) kind: TabKind,
    pub(crate) leader: TabLeader,
}

/// The tab stops of a paragraph.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TabStops {
    /// The stops a tab can take the text to, in order: the custom stops but bar tabs,
    /// and the stop a hanging indent makes.
    stops: Vec<Stop>,
    /// The distance between the automatic stops after the last of `stops`.
    interval: Pt,
}

impl TabStops {
    /// The tab stops of a paragraph with `props` in a document with `settings`;
    /// `numbered` when the paragraph is an item of a list.
    ///
    /// A hanging indent is a stop at the indent of the lines after the first, for
    /// paragraphs of every kind: Word's renders of `E-tab-hanging.docx` show a typed
    /// `1.1<TAB>` going there before the automatic stop it passes. The legacy option
    /// `doNotUseIndentAsNumberingTabStop` takes that stop away from list items only
    /// (`E-tab-hanging-noindenttab-14.docx`), and in mode 15 Word drops the option when
    /// it opens the document and lays it out as if it were absent
    /// (`E-tab-hanging-noindenttab.docx`).
    pub(crate) fn new(props: &ParagraphProps, settings: &DocumentSettings, numbered: bool) -> Self {
        let mut stops: Vec<Stop> = props
            .tabs()
            .iter()
            .filter(|stop| stop.kind() != TabKind::Bar)
            .map(|stop| Stop {
                position: stop.position(),
                kind: stop.kind(),
                leader: stop.leader(),
            })
            .collect();
        let indent = props.indent();
        let flag_applies = numbered
            && settings.compat_mode() < CompatMode::Word2013
            && settings.has_compat_flag(&CompatFlag::DoNotUseIndentAsNumberingTabStop);
        if indent.first_line().0 < 0.0 && !flag_applies {
            let position = indent.start();
            if !stops.iter().any(|stop| stop.position == position) {
                stops.push(Stop {
                    position,
                    kind: TabKind::Start,
                    leader: TabLeader::None,
                });
            }
        }
        stops.sort_by(|a, b| a.position.0.total_cmp(&b.position.0));
        Self {
            stops,
            interval: settings.default_tab_stop(),
        }
    }

    /// The stop a tab at `x` takes the text to: the first stop after `x` (a stop at `x`
    /// itself is passed by), then the automatic stops after the last stop, at the
    /// multiples of the interval from the start edge of the column (the edge itself
    /// for text in the margin). `None` when there is no stop after `x`,
    /// which happens only when the interval is zero.
    pub(crate) fn next(&self, x: Pt) -> Option<Stop> {
        if let Some(stop) = self.stops.iter().find(|stop| stop.position.0 > x.0) {
            return Some(*stop);
        }
        if self.interval.0 <= 0.0 {
            return None;
        }
        let last = self
            .stops
            .last()
            .map_or(f64::NEG_INFINITY, |stop| stop.position.0);
        let from = x.0.max(last);
        // The first multiple of the interval after `from`.
        let mut position = ((from / self.interval.0).floor() + 1.0) * self.interval.0;
        // Rounding can leave the product at `from` itself.
        if position <= from {
            position += self.interval.0;
        }
        Some(Stop {
            position: Pt(position),
            kind: TabKind::Start,
            leader: TabLeader::None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Indent, TabStop};

    fn hanging(start: f64, hang: f64) -> ParagraphProps {
        ParagraphProps::default().with_indent(
            Indent::default()
                .with_start(Pt(start))
                .with_first_line(Pt(-hang)),
        )
    }

    fn positions(stops: &TabStops, xs: &[f64]) -> Vec<Option<f64>> {
        xs.iter()
            .map(|&x| stops.next(Pt(x)).map(|stop| stop.position.0))
            .collect()
    }

    #[test]
    fn a_tab_goes_to_the_next_custom_stop_then_to_automatic_stops() {
        let props = ParagraphProps::default().with_tabs([
            TabStop::default().with_position(Pt(100.0)),
            TabStop::default()
                .with_position(Pt(50.0))
                .with_kind(TabKind::End)
                .with_leader(TabLeader::Dot),
        ]);
        let stops = TabStops::new(&props, &DocumentSettings::default(), false);
        // A stop at the tab's own place is passed by; automatic stops every 36 pt come
        // only after the last custom stop.
        assert_eq!(
            positions(&stops, &[0.0, 50.0, 60.0, 100.0, 107.9, 108.0]),
            [
                Some(50.0),
                Some(100.0),
                Some(100.0),
                Some(108.0),
                Some(108.0),
                Some(144.0)
            ]
        );
        let end = stops.next(Pt(0.0));
        assert_eq!(
            end.map(|stop| (stop.kind, stop.leader)),
            Some((TabKind::End, TabLeader::Dot))
        );
    }

    #[test]
    fn a_hanging_indent_is_a_stop_before_the_automatic_ones() {
        // E-tab-hanging, case A: "1.1" then a tab, hanging indent 50 pt: the text goes
        // to 50 pt, not to the automatic stop at 36 pt. Case F: indent 100, hanging 50.
        let settings = DocumentSettings::default();
        let stops = TabStops::new(&hanging(50.0, 50.0), &settings, false);
        assert_eq!(
            positions(&stops, &[13.5, 50.0, 68.0]),
            [Some(50.0), Some(72.0), Some(72.0)]
        );
        let stops = TabStops::new(&hanging(100.0, 50.0), &settings, false);
        assert_eq!(positions(&stops, &[63.0]), [Some(100.0)]);
        // Case D: a custom stop at 75 pt after the indent's stop at 50 pt.
        let props = hanging(50.0, 50.0).with_tabs([TabStop::default().with_position(Pt(75.0))]);
        let stops = TabStops::new(&props, &settings, false);
        assert_eq!(positions(&stops, &[13.5, 50.0]), [Some(50.0), Some(75.0)]);
        // A first-line indent makes no stop (case E).
        let props =
            ParagraphProps::default().with_indent(Indent::default().with_first_line(Pt(25.0)));
        let stops = TabStops::new(&props, &settings, false);
        assert_eq!(positions(&stops, &[38.0]), [Some(72.0)]);
    }

    #[test]
    fn the_legacy_option_takes_the_indent_stop_from_list_items_before_mode_15() {
        let props = hanging(50.0, 50.0);
        let flagged = |mode| {
            DocumentSettings::default()
                .with_compat_mode(mode)
                .with_compat_flags([CompatFlag::DoNotUseIndentAsNumberingTabStop])
        };
        // E-tab-hanging-noindenttab-14, case B: a list label goes to the automatic stop
        // at 36 pt; case A, a typed label, still to the indent.
        let mode_14 = flagged(CompatMode::Word2010);
        assert_eq!(
            positions(&TabStops::new(&props, &mode_14, true), &[9.0]),
            [Some(36.0)]
        );
        assert_eq!(
            positions(&TabStops::new(&props, &mode_14, false), &[9.0]),
            [Some(50.0)]
        );
        // In mode 15 the option does nothing (E-tab-hanging-noindenttab).
        let mode_15 = flagged(CompatMode::Word2013);
        assert_eq!(
            positions(&TabStops::new(&props, &mode_15, true), &[9.0]),
            [Some(50.0)]
        );
    }

    #[test]
    fn bar_tabs_take_no_text_and_a_zero_interval_has_no_automatic_stops() {
        let props = ParagraphProps::default().with_tabs([TabStop::default()
            .with_position(Pt(20.0))
            .with_kind(TabKind::Bar)]);
        let settings = DocumentSettings::default().with_default_tab_stop(Pt(0.0));
        let stops = TabStops::new(&props, &settings, false);
        assert_eq!(positions(&stops, &[0.0]), [None]);
        // Stops before the start edge are stops too; ones at the same place are one.
        let props = hanging(-10.0, 10.0).with_tabs([
            TabStop::default().with_position(Pt(-30.0)),
            TabStop::default().with_position(Pt(-10.0)),
        ]);
        let stops = TabStops::new(&props, &DocumentSettings::default(), false);
        assert_eq!(stops.stops.len(), 2);
        // The start edge is an automatic stop for text in the margin.
        assert_eq!(
            positions(&stops, &[-40.0, -30.0, -10.0, 0.0]),
            [Some(-30.0), Some(-10.0), Some(0.0), Some(36.0)]
        );
    }
}
