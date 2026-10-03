//! Shared low-level helpers: JavaScript string and JSON semantics, Node-style paths and
//! filesystem calls, and the process environment as a value.
//!
//! Every module reaches the filesystem, the environment, and string ordering through here, so the
//! behaviour the TypeScript build had (UTF-16 ordering, lossy UTF-8 reads, lexical path
//! normalization, ENOENT-only absence) is decided once. `clippy.toml` forbids the std calls this
//! module wraps everywhere else.

pub mod cmp;
pub mod env;
pub mod fs;
pub mod hash;
pub mod json;
pub mod path;
pub mod text;

/// Declares an enum for a TypeScript string-literal union.
///
/// Variants are listed in the TS array's order, which is also the derived `Ord`, so sorting by kind
/// sorts as the reports list them. Each variant names its TS spelling once; `as_str`, `parse`,
/// `Display` and serde all use it. `ALL` is the TS array (`ITEM_KINDS`, `HOOK_EVENTS`, …).
macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $($(#[$vmeta:meta])* $variant:ident => $text:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord,
            serde::Serialize, serde::Deserialize,
        )]
        $vis enum $name {
            $($(#[$vmeta])* #[serde(rename = $text)] $variant),+
        }

        impl $name {
            /// Every variant, in declaration order.
            #[allow(dead_code)] // the macro serves every union; not each one is enumerated
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// The TypeScript spelling.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }

            /// The variant spelled `text`, if any.
            #[allow(dead_code)] // the macro serves every union; not each one is parsed from text
            pub fn parse(text: &str) -> Option<Self> {
                match text {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

pub(crate) use string_enum;
