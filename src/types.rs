//! Semantic types.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type {
    Int,
    Float,
    Bool,
    Str,
    Void,
    /// Type of expressions that never produce a value (`return`, `break`,
    /// `panic(...)`). Coerces to any other type.
    Never,
    Array(Box<Type>),
    Struct(String),
    Enum(String),
    Fn(Vec<Type>, Box<Type>),
    /// Not yet inferred (e.g. element type of `[]`).
    Unknown,
    /// Result of an erroneous expression; suppresses cascading errors.
    Error,
}

impl Type {
    pub fn is_never(&self) -> bool {
        matches!(self, Type::Never)
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Type::Error)
    }

    /// True if values of this type can be stored in a variable.
    pub fn is_value(&self) -> bool {
        !matches!(self, Type::Void | Type::Never | Type::Unknown)
    }

    /// Contains `Unknown` somewhere inside (e.g. `[?]`).
    pub fn has_unknown(&self) -> bool {
        match self {
            Type::Unknown => true,
            Type::Array(t) => t.has_unknown(),
            Type::Fn(ps, r) => ps.iter().any(|p| p.has_unknown()) || r.has_unknown(),
            _ => false,
        }
    }

    /// Values that can be printed / interpolated.
    pub fn is_printable(&self) -> bool {
        matches!(
            self,
            Type::Int | Type::Float | Type::Bool | Type::Str | Type::Enum(_)
        )
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Int => write!(f, "int"),
            Type::Float => write!(f, "float"),
            Type::Bool => write!(f, "bool"),
            Type::Str => write!(f, "str"),
            Type::Void => write!(f, "void"),
            Type::Never => write!(f, "never"),
            Type::Array(t) => write!(f, "[{t}]"),
            Type::Struct(n) | Type::Enum(n) => write!(f, "{n}"),
            Type::Fn(ps, r) => {
                write!(f, "fn(")?;
                for (i, p) in ps.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{p}")?;
                }
                write!(f, ")")?;
                if **r != Type::Void {
                    write!(f, " -> {r}")?;
                }
                Ok(())
            }
            Type::Unknown => write!(f, "?"),
            Type::Error => write!(f, "<ошибка>"),
        }
    }
}
