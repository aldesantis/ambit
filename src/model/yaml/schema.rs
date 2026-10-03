//! YAML 1.2 core schema resolution, as yaml@2.9.0's `core` schema does it.
//!
//! The same tests decide both halves: [`resolve_scalar`] types a plain scalar by the first test it
//! passes, and the emitter quotes any string that passes one, so what ambit writes reads back as
//! the string it was.

use std::sync::LazyLock;

use regex::Regex;

/// The prefix the `!!` handle expands to.
pub(super) const CORE_PREFIX: &str = "tag:yaml.org,2002:";

pub(super) const STR_TAG: &str = "tag:yaml.org,2002:str";

/// The tags the YAML 1.2 core schema resolves. A node carrying anything else is using a custom
/// tag, and the document is rejected: arbitrary type resolution is how `!!python/object`
/// constructs get in.
pub(super) const CORE_TAGS: &[&str] = &[
    "tag:yaml.org,2002:bool",
    "tag:yaml.org,2002:float",
    "tag:yaml.org,2002:int",
    "tag:yaml.org,2002:map",
    "tag:yaml.org,2002:null",
    "tag:yaml.org,2002:seq",
    "tag:yaml.org,2002:str",
];

/// A resolved scalar. Numbers are `f64` because the TypeScript build read them as JavaScript
/// numbers: "is an integer" means a finite value with no fraction, as `Number.isInteger` has it.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Scalar {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
}

#[derive(Clone, Copy)]
enum Resolver {
    Null,
    Bool,
    Int { offset: usize, radix: u32 },
    FloatNan,
    Float,
}

struct CoreTag {
    tag: &'static str,
    test: &'static LazyLock<Regex>,
    resolver: Resolver,
}

macro_rules! test {
    ($name:ident, $pattern:literal) => {
        static $name: LazyLock<Regex> =
            LazyLock::new(|| Regex::new($pattern).expect("a valid core schema pattern"));
    };
}

test!(NULL_TEST, r"^(?:~|[Nn]ull|NULL)?$");
test!(BOOL_TEST, r"^(?:[Tt]rue|TRUE|[Ff]alse|FALSE)$");
test!(INT_OCT_TEST, r"^0o[0-7]+$");
test!(INT_TEST, r"^[-+]?[0-9]+$");
test!(INT_HEX_TEST, r"^0x[0-9a-fA-F]+$");
test!(
    FLOAT_NAN_TEST,
    r"^(?:[-+]?\.(?:inf|Inf|INF)|\.nan|\.NaN|\.NAN)$"
);
test!(
    FLOAT_EXP_TEST,
    r"^[-+]?(?:\.[0-9]+|[0-9]+(?:\.[0-9]*)?)[eE][-+]?[0-9]+$"
);
test!(FLOAT_TEST, r"^[-+]?(?:\.[0-9]+|[0-9]+\.[0-9]*)$");

/// The schema's tests in yaml's order; the first one a plain scalar passes types it.
static TAGS: [CoreTag; 8] = [
    CoreTag {
        tag: "tag:yaml.org,2002:null",
        test: &NULL_TEST,
        resolver: Resolver::Null,
    },
    CoreTag {
        tag: "tag:yaml.org,2002:bool",
        test: &BOOL_TEST,
        resolver: Resolver::Bool,
    },
    CoreTag {
        tag: "tag:yaml.org,2002:int",
        test: &INT_OCT_TEST,
        resolver: Resolver::Int {
            offset: 2,
            radix: 8,
        },
    },
    CoreTag {
        tag: "tag:yaml.org,2002:int",
        test: &INT_TEST,
        resolver: Resolver::Int {
            offset: 0,
            radix: 10,
        },
    },
    CoreTag {
        tag: "tag:yaml.org,2002:int",
        test: &INT_HEX_TEST,
        resolver: Resolver::Int {
            offset: 2,
            radix: 16,
        },
    },
    CoreTag {
        tag: "tag:yaml.org,2002:float",
        test: &FLOAT_NAN_TEST,
        resolver: Resolver::FloatNan,
    },
    CoreTag {
        tag: "tag:yaml.org,2002:float",
        test: &FLOAT_EXP_TEST,
        resolver: Resolver::Float,
    },
    CoreTag {
        tag: "tag:yaml.org,2002:float",
        test: &FLOAT_TEST,
        resolver: Resolver::Float,
    },
];

/// Whether `text`, written plain, would read back as something other than a string.
pub(super) fn resolves_to_non_string(text: &str) -> bool {
    TAGS.iter().any(|tag| tag.test.is_match(text))
}

/// Types a scalar. `plain` is whether it was written without quotes or a block indicator; `tag`
/// is its explicit tag, fully expanded.
///
/// Only a plain scalar is typed by the schema's tests. An explicit core tag picks among its own
/// tests instead, and falls back to a string when none passes: `!!float 1` is the string `"1"`,
/// as yaml (which only warns about it) reads it.
pub(super) fn resolve_scalar(value: &str, plain: bool, tag: Option<&str>) -> Scalar {
    let found = match tag {
        Some(tag) => TAGS
            .iter()
            .find(|core| core.tag == tag && core.test.is_match(value)),
        None if plain => TAGS.iter().find(|core| core.test.is_match(value)),
        None => None,
    };

    match found {
        None => Scalar::String(value.to_owned()),
        Some(core) => match core.resolver {
            Resolver::Null => Scalar::Null,
            Resolver::Bool => Scalar::Bool(value.starts_with(['t', 'T'])),
            Resolver::Int { offset, radix } => Scalar::Number(parse_int(&value[offset..], radix)),
            Resolver::FloatNan => Scalar::Number(if value.to_lowercase().ends_with("nan") {
                f64::NAN
            } else if value.starts_with('-') {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            }),
            Resolver::Float => Scalar::Number(value.parse().unwrap_or(f64::NAN)),
        },
    }
}

/// `parseInt(digits, radix)` for text that already passed its test.
fn parse_int(digits: &str, radix: u32) -> f64 {
    if radix == 10 {
        return digits.parse().unwrap_or(f64::NAN);
    }

    digits
        .chars()
        .filter_map(|c| c.to_digit(radix))
        .fold(0.0, |total, digit| {
            total * f64::from(radix) + f64::from(digit)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(value: &str) -> f64 {
        match resolve_scalar(value, true, None) {
            Scalar::Number(n) => n,
            other => panic!("expected a number for {value}, got {other:?}"),
        }
    }

    #[test]
    fn types_plain_scalars_by_the_core_schema() {
        assert_eq!(resolve_scalar("", true, None), Scalar::Null);
        assert_eq!(resolve_scalar("~", true, None), Scalar::Null);
        assert_eq!(resolve_scalar("NULL", true, None), Scalar::Null);
        assert_eq!(resolve_scalar("True", true, None), Scalar::Bool(true));
        assert_eq!(resolve_scalar("FALSE", true, None), Scalar::Bool(false));
        assert_eq!(
            resolve_scalar("yes", true, None),
            Scalar::String("yes".into())
        );
        assert!((number("1e5") - 100_000.0).abs() < f64::EPSILON);
        assert!((number("0x1F") - 31.0).abs() < f64::EPSILON);
        assert!((number("0o17") - 15.0).abs() < f64::EPSILON);
        assert!((number("007") - 7.0).abs() < f64::EPSILON);
        assert!((number("+12") - 12.0).abs() < f64::EPSILON);
        assert!((number(".5") - 0.5).abs() < f64::EPSILON);
        assert!((number("1.") - 1.0).abs() < f64::EPSILON);
        assert!(number(".nan").is_nan());
        assert_eq!(number("-.inf"), f64::NEG_INFINITY);
        assert_eq!(
            resolve_scalar("1_000", true, None),
            Scalar::String("1_000".into())
        );
    }

    #[test]
    fn leaves_quoted_scalars_strings() {
        assert_eq!(resolve_scalar("1", false, None), Scalar::String("1".into()));
    }

    #[test]
    fn lets_an_explicit_tag_pick_among_its_own_tests() {
        assert_eq!(
            resolve_scalar("1234567", true, Some(STR_TAG)),
            Scalar::String("1234567".into())
        );
        assert_eq!(
            resolve_scalar("12", false, Some("tag:yaml.org,2002:int")),
            Scalar::Number(12.0)
        );
        assert_eq!(
            resolve_scalar("1", true, Some("tag:yaml.org,2002:float")),
            Scalar::String("1".into())
        );
    }
}
