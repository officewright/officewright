//! The two walks over the IR: [`Check`] for `Document::validate` and [`Clamp`] for
//! `Document::clamp`. The structs implement both through `ir_struct!`, which visits
//! every field; the values at the leaves are here.

use officewright_core::{Color, PageSize, Pt};

use super::macros::no_rules;
use super::{Document, RunStyleId};
use crate::report::{LayoutReport, Location, ReportKind, Step};
use crate::{Error, Result};

/// A part of the IR that `Document::validate` checks: lengths are finite numbers, ids
/// belong to the document, and the rules of the IR hold.
pub(crate) trait Check {
    fn check(&self, checker: &mut Checker<'_>) -> Result<()>;
}

/// A part of the IR in which `Document::clamp` brings values within Word's limits.
pub(crate) trait Clamp {
    fn clamp(&mut self, clamper: &mut Clamper);
}

/// The state of a `Document::validate` walk.
pub(crate) struct Checker<'a> {
    document: &'a Document,
    location: Location,
    /// Whether the walk is inside the stored result of a field.
    in_field_result: bool,
}

impl<'a> Checker<'a> {
    pub(crate) fn new(document: &'a Document) -> Self {
        Self {
            document,
            location: Location::default(),
            in_field_result: false,
        }
    }

    pub(crate) fn document(&self) -> &'a Document {
        self.document
    }

    /// Steps into a field of the value being checked.
    pub(crate) fn enter(&mut self, field: &'static str) {
        self.location.push(Step::Field(field));
    }

    /// Steps back out of the field last entered.
    pub(crate) fn leave(&mut self) {
        self.location.pop();
    }

    /// Checks a field.
    pub(crate) fn visit<T: Check + ?Sized>(
        &mut self,
        field: &'static str,
        value: &T,
    ) -> Result<()> {
        self.enter(field);
        value.check(self)?;
        self.leave();
        Ok(())
    }

    pub(crate) fn in_field_result(&self) -> bool {
        self.in_field_result
    }

    pub(crate) fn set_in_field_result(&mut self, value: bool) {
        self.in_field_result = value;
    }

    /// The error for a problem with the value being checked.
    pub(crate) fn error(&self, problem: impl Into<String>) -> Error {
        Error::InvalidDocument {
            location: self.location.clone(),
            problem: problem.into(),
        }
    }
}

/// The state of a `Document::clamp` walk.
#[derive(Default)]
pub(crate) struct Clamper {
    location: Location,
    report: LayoutReport,
}

impl Clamper {
    /// Steps into a field of the value being clamped.
    pub(crate) fn enter(&mut self, field: &'static str) {
        self.location.push(Step::Field(field));
    }

    /// Steps back out of the field last entered.
    pub(crate) fn leave(&mut self) {
        self.location.pop();
    }

    /// Clamps a field.
    pub(crate) fn visit<T: Clamp + ?Sized>(&mut self, field: &'static str, value: &mut T) {
        self.enter(field);
        value.clamp(self);
        self.leave();
    }

    /// Records that the value being clamped was changed.
    pub(crate) fn clamped(&mut self, message: String) {
        self.report
            .push(ReportKind::ValueClamped, self.location.clone(), message);
    }

    pub(crate) fn finish(self) -> LayoutReport {
        self.report
    }
}

impl Check for Pt {
    fn check(&self, checker: &mut Checker<'_>) -> Result<()> {
        if self.0.is_finite() {
            Ok(())
        } else {
            Err(checker.error(format!("{} is not a finite length", self.0)))
        }
    }
}

impl Check for PageSize {
    fn check(&self, checker: &mut Checker<'_>) -> Result<()> {
        checker.visit("width", &self.width())?;
        checker.visit("height", &self.height())
    }
}

impl Check for RunStyleId {
    fn check(&self, checker: &mut Checker<'_>) -> Result<()> {
        checker.document().check_run_style(*self, checker)
    }
}

impl<T: Check> Check for Option<T> {
    fn check(&self, checker: &mut Checker<'_>) -> Result<()> {
        match self {
            Some(value) => value.check(checker),
            None => Ok(()),
        }
    }
}

impl<T: Check> Check for Vec<T> {
    fn check(&self, checker: &mut Checker<'_>) -> Result<()> {
        for (index, value) in self.iter().enumerate() {
            checker.location.push(Step::Index(index));
            value.check(checker)?;
            checker.location.pop();
        }
        Ok(())
    }
}

impl<T: Clamp> Clamp for Option<T> {
    fn clamp(&mut self, clamper: &mut Clamper) {
        if let Some(value) = self {
            value.clamp(clamper);
        }
    }
}

impl<T: Clamp> Clamp for Vec<T> {
    fn clamp(&mut self, clamper: &mut Clamper) {
        for (index, value) in self.iter_mut().enumerate() {
            clamper.location.push(Step::Index(index));
            value.clamp(clamper);
            clamper.location.pop();
        }
    }
}

impl Clamp for Pt {
    fn clamp(&mut self, _: &mut Clamper) {}
}

impl Clamp for PageSize {
    fn clamp(&mut self, _: &mut Clamper) {}
}

impl Clamp for RunStyleId {
    fn clamp(&mut self, _: &mut Clamper) {}
}

no_rules!(bool, u8, u16, u32, char, String, Color);
