//! The macros that define the structs of the IR.

/// Defines a struct of the IR: private fields, and for each field a getter and a `with_*`
/// setter that both carry the field's documentation. It also implements
/// [`Check`](super::walk::Check) and [`Clamp`](super::walk::Clamp) by visiting every field,
/// so that `Document::validate` and `Document::clamp` cannot miss one.
///
/// Each field is written as `kind name: Type [= default] => with_name [, check rule]
/// [, clamp rule];`. The kind says how the field is stored and read:
///
/// | Kind | Stored as | Getter returns | Setter takes |
/// |---|---|---|---|
/// | `copy T` | `T` | `T` | `T` |
/// | `ref T` | `T` | `&T` | `T` |
/// | `str String` | `String` | `&str` | `impl Into<String>` |
/// | `opt_str Option<String>` | `Option<String>` | `Option<&str>` | `Option<String>` |
/// | `vec T` | `Vec<T>` | `&[T]` | `impl IntoIterator<Item = T>` |
///
/// When every field has a default, the struct implements `Default` with them. A `check`
/// rule is a function `fn(&Stored, &mut Checker) -> Result<()>` that `validate` runs on the
/// field after its own checks; a `clamp` rule is a function `fn(&mut Stored, &mut
/// Clamper)` that `clamp` runs on it. A `check rule;` or `clamp rule;` after the struct
/// runs on the whole struct, after its fields.
macro_rules! ir_struct {
    (@storage copy $ty:ty) => { $ty };
    (@storage ref $ty:ty) => { $ty };
    (@storage str $ty:ty) => { $ty };
    (@storage opt_str $ty:ty) => { $ty };
    (@storage vec $ty:ty) => { Vec<$ty> };

    (@get_type copy $ty:ty) => { $ty };
    (@get_type ref $ty:ty) => { &$ty };
    (@get_type str $ty:ty) => { &str };
    (@get_type opt_str $ty:ty) => { Option<&str> };
    (@get_type vec $ty:ty) => { &[$ty] };

    (@get copy $value:expr) => { $value };
    (@get ref $value:expr) => { &$value };
    (@get str $value:expr) => { &$value };
    (@get opt_str $value:expr) => { $value.as_deref() };
    (@get vec $value:expr) => { &$value };

    (@set_type copy $ty:ty) => { $ty };
    (@set_type ref $ty:ty) => { $ty };
    (@set_type str $ty:ty) => { impl Into<$ty> };
    (@set_type opt_str $ty:ty) => { $ty };
    (@set_type vec $ty:ty) => { impl IntoIterator<Item = $ty> };

    (@set copy $value:expr) => { $value };
    (@set ref $value:expr) => { $value };
    (@set str $value:expr) => { $value.into() };
    (@set opt_str $value:expr) => { $value };
    (@set vec $value:expr) => { $value.into_iter().collect() };

    (
        @struct
        $(#[$meta:meta])*
        $name:ident {
            $(
                $(#[doc = $doc:expr])*
                $kind:ident $field:ident: $ty:ty => $setter:ident
                    $(, check $check:path)? $(, clamp $clamp:path)?;
            )*
        }
        $(check $struct_check:path;)?
        $(clamp $struct_clamp:path;)?
    ) => {
        $(#[$meta])*
        #[non_exhaustive]
        pub struct $name {
            $(
                $(#[doc = $doc])*
                $field: ir_struct!(@storage $kind $ty),
            )*
        }

        impl $name {
            $(
                $(#[doc = $doc])*
                pub fn $field(&self) -> ir_struct!(@get_type $kind $ty) {
                    ir_struct!(@get $kind self.$field)
                }

                #[doc = concat!("Sets [`", stringify!($field), "`](Self::", stringify!($field), ").")]
                #[doc = ""]
                $(#[doc = $doc])*
                pub fn $setter(mut self, value: ir_struct!(@set_type $kind $ty)) -> Self {
                    self.$field = ir_struct!(@set $kind value);
                    self
                }
            )*
        }

        impl $crate::ir::walk::Check for $name {
            fn check(
                &self,
                checker: &mut $crate::ir::walk::Checker<'_>,
            ) -> $crate::Result<()> {
                $(
                    checker.enter(stringify!($field));
                    $crate::ir::walk::Check::check(&self.$field, checker)?;
                    $($check(&self.$field, checker)?;)?
                    checker.leave();
                )*
                $($struct_check(self, checker)?;)?
                Ok(())
            }
        }

        impl $crate::ir::walk::Clamp for $name {
            fn clamp(&mut self, clamper: &mut $crate::ir::walk::Clamper) {
                $(
                    clamper.enter(stringify!($field));
                    $crate::ir::walk::Clamp::clamp(&mut self.$field, clamper);
                    $($clamp(&mut self.$field, clamper);)?
                    clamper.leave();
                )*
                $($struct_clamp(self, clamper);)?
            }
        }
    };

    // Every field has a default.
    (
        $(#[$meta:meta])*
        pub struct $name:ident {
            $(
                $(#[doc = $doc:expr])*
                $kind:ident $field:ident: $ty:ty = $default:expr => $setter:ident
                    $(, check $check:path)? $(, clamp $clamp:path)?;
            )*
        }
        $(check $struct_check:path;)?
        $(clamp $struct_clamp:path;)?
    ) => {
        ir_struct! {
            @struct
            $(#[$meta])*
            $name {
                $(
                    $(#[doc = $doc])*
                    $kind $field: $ty => $setter $(, check $check)? $(, clamp $clamp)?;
                )*
            }
            $(check $struct_check;)?
            $(clamp $struct_clamp;)?
        }

        impl Default for $name {
            fn default() -> Self {
                Self {
                    $($field: $default,)*
                }
            }
        }
    };

    // Some field has no default: the struct gets a constructor of its own.
    (
        $(#[$meta:meta])*
        pub struct $name:ident {
            $(
                $(#[doc = $doc:expr])*
                $kind:ident $field:ident: $ty:ty => $setter:ident
                    $(, check $check:path)? $(, clamp $clamp:path)?;
            )*
        }
        $(check $struct_check:path;)?
        $(clamp $struct_clamp:path;)?
    ) => {
        ir_struct! {
            @struct
            $(#[$meta])*
            $name {
                $(
                    $(#[doc = $doc])*
                    $kind $field: $ty => $setter $(, check $check)? $(, clamp $clamp)?;
                )*
            }
            $(check $struct_check;)?
            $(clamp $struct_clamp;)?
        }
    };
}
pub(crate) use ir_struct;

/// Implements [`Check`](super::walk::Check) and [`Clamp`](super::walk::Clamp) for values
/// that hold nothing to check or clamp: plain enums, flags, counts.
macro_rules! no_rules {
    ($($ty:ty),* $(,)?) => {
        $(
            impl $crate::ir::walk::Check for $ty {
                fn check(&self, _: &mut $crate::ir::walk::Checker<'_>) -> $crate::Result<()> {
                    Ok(())
                }
            }

            impl $crate::ir::walk::Clamp for $ty {
                fn clamp(&mut self, _: &mut $crate::ir::walk::Clamper) {}
            }
        )*
    };
}
pub(crate) use no_rules;
