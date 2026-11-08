//! Abstract syntax tree.
//!
//! The parser produces the "surface" forms. The type checker annotates every
//! expression with its [`Type`] and rewrites name-dependent nodes into resolved
//! forms (marked "resolved" below), so the back ends never look names up.

use crate::diag::Span;
use crate::types::Type;

pub type LocalId = u32;

// ── Types as written in source ────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum TypeExpr {
    Named(String, Span),
    Array(Box<TypeExpr>, Span),
    Fn(Vec<TypeExpr>, Option<Box<TypeExpr>>, Span),
}

// ── Items ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Item {
    pub kind: ItemKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ItemKind {
    Import(String),
    Const(ConstDecl),
    Fn(FnDecl),
    Extern(ExternDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    Impl(ImplDecl),
}

#[derive(Debug, Clone)]
pub struct ConstDecl {
    pub name: String,
    pub ty: Option<TypeExpr>,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FnDecl {
    pub name: String,
    pub has_self: bool,
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    pub body: Block,
    pub span: Span,
    // Filled by the checker:
    /// Fully qualified name: `foo`, `Point::len`, `lambda#3`.
    pub symbol: String,
    /// All locals of the function; params (and `self`) come first.
    pub locals: Vec<LocalInfo>,
    pub ret_ty: Type,
}

#[derive(Debug, Clone)]
pub struct LocalInfo {
    pub name: String,
    pub ty: Type,
    pub mutable: bool,
}

#[derive(Debug, Clone)]
pub struct ExternDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
}

#[derive(Debug, Clone)]
pub struct StructDecl {
    pub name: String,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub name: String,
    pub variants: Vec<(String, Span)>,
}

#[derive(Debug, Clone)]
pub struct ImplDecl {
    pub type_name: String,
    pub methods: Vec<FnDecl>,
}

// ── Statements ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    /// Trailing expression without `;` — the value of the block.
    pub tail: Option<Box<Expr>>,
    pub span: Span,
    /// Line of the closing `}` (used by the formatter to place comments).
    pub end_line: u32,
}

#[derive(Debug, Clone)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    Let {
        name: String,
        mutable: bool,
        ty: Option<TypeExpr>,
        init: Expr,
        local: LocalId,
    },
    Assign {
        target: Expr,
        op: Option<BinOp>,
        value: Expr,
    },
    /// Expression statement. `semi` is false for block-like expressions
    /// (`if`, `match`, loops, blocks) written without a trailing `;`.
    Expr {
        expr: Expr,
        semi: bool,
    },
    Defer(Expr),
}

// ── Expressions ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Pow => "**",
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
        }
    }

    /// Binding power: higher binds tighter.
    pub fn precedence(self) -> u8 {
        match self {
            BinOp::Or => 2,
            BinOp::And => 3,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 4,
            BinOp::BitOr => 5,
            BinOp::BitXor => 6,
            BinOp::BitAnd => 7,
            BinOp::Shl | BinOp::Shr => 8,
            BinOp::Add | BinOp::Sub => 9,
            BinOp::Mul | BinOp::Div | BinOp::Rem => 10,
            BinOp::Pow => 12,
        }
    }
}

/// Precedence of the pipe operator `|>` (lowest of all binary operators).
pub const PIPE_PREC: u8 = 1;
/// Precedence of `as` casts and unary operators.
pub const UNARY_PREC: u8 = 11;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    BitNot,
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
    pub ty: Type,
}

impl Expr {
    pub fn new(kind: ExprKind, span: Span) -> Self {
        Expr {
            kind,
            span,
            ty: Type::Unknown,
        }
    }

    /// Block-like expressions may be used as statements without `;`.
    pub fn is_block_like(&self) -> bool {
        matches!(
            self.kind,
            ExprKind::If { .. }
                | ExprKind::Match { .. }
                | ExprKind::Block(_)
                | ExprKind::While { .. }
                | ExprKind::For { .. }
                | ExprKind::Loop(_)
        )
    }
}

#[derive(Debug, Clone)]
pub enum InterpPart {
    Lit(String),
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub enum ForIter {
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        inclusive: bool,
    },
    Each(Box<Expr>),
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pats: Vec<Pat>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Pat {
    Wildcard(Span),
    Int(i64, Span),
    Str(String, Span),
    Bool(bool, Span),
    Range(i64, i64, bool, Span),
    /// `Enum::Variant`, resolved to the variant index by the checker.
    Variant(String, String, Span, u32),
}

impl Pat {
    pub fn span(&self) -> Span {
        match self {
            Pat::Wildcard(s)
            | Pat::Int(_, s)
            | Pat::Str(_, s)
            | Pat::Bool(_, s)
            | Pat::Range(_, _, _, s)
            | Pat::Variant(_, _, s, _) => *s,
        }
    }
}

/// Built-in functions and methods known to the compiler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Print,
    Println,
    Eprintln,
    ReadLine,
    ParseInt,
    ParseFloat,
    Chr,
    Exit,
    Panic,
    Assert,
    Sqrt,
    Sin,
    Cos,
    Tan,
    Atan2,
    Exp,
    Ln,
    Floor,
    Ceil,
    Round,
    Abs,
    Min,
    Max,
    Clock,
    // array methods (receiver is args[0])
    ArrLen,
    ArrPush,
    ArrPop,
    // string methods (receiver is args[0])
    StrLen,
    StrByte,
    StrSlice,
    StrContains,
    StrFind,
}

impl Builtin {
    pub fn function(name: &str) -> Option<Builtin> {
        Some(match name {
            "print" => Builtin::Print,
            "println" => Builtin::Println,
            "eprintln" => Builtin::Eprintln,
            "read_line" => Builtin::ReadLine,
            "parse_int" => Builtin::ParseInt,
            "parse_float" => Builtin::ParseFloat,
            "chr" => Builtin::Chr,
            "exit" => Builtin::Exit,
            "panic" => Builtin::Panic,
            "assert" => Builtin::Assert,
            "sqrt" => Builtin::Sqrt,
            "sin" => Builtin::Sin,
            "cos" => Builtin::Cos,
            "tan" => Builtin::Tan,
            "atan2" => Builtin::Atan2,
            "exp" => Builtin::Exp,
            "ln" => Builtin::Ln,
            "floor" => Builtin::Floor,
            "ceil" => Builtin::Ceil,
            "round" => Builtin::Round,
            "abs" => Builtin::Abs,
            "min" => Builtin::Min,
            "max" => Builtin::Max,
            "clock" => Builtin::Clock,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    // ── Surface forms ──
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Interp(Vec<InterpPart>),
    Ident(String),
    SelfRef,
    /// `Type::name` — enum variant or static method.
    Path(String, String),
    Paren(Box<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// `lhs |> f` / `lhs |> f(args)`.
    Pipe(Box<Expr>, Box<Expr>),
    Cast(Box<Expr>, TypeExpr),
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    MethodCall {
        recv: Box<Expr>,
        method: String,
        args: Vec<Expr>,
    },
    Field {
        obj: Box<Expr>,
        name: String,
    },
    Index {
        obj: Box<Expr>,
        idx: Box<Expr>,
    },
    StructLit {
        name: String,
        fields: Vec<(String, Expr)>,
    },
    ArrayLit(Vec<Expr>),
    ArrayRepeat {
        value: Box<Expr>,
        count: Box<Expr>,
    },
    Lambda {
        params: Vec<Param>,
        ret: Option<TypeExpr>,
        body: Block,
    },
    If {
        cond: Box<Expr>,
        then: Block,
        els: Option<Box<Expr>>,
    },
    Match {
        scrut: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    Block(Block),
    While {
        cond: Box<Expr>,
        body: Block,
    },
    For {
        var: String,
        local: LocalId,
        iter: ForIter,
        body: Block,
    },
    Loop(Block),
    Break,
    Continue,
    Return(Option<Box<Expr>>),

    // ── Resolved forms (produced by the checker) ──
    Local(LocalId),
    /// Reference to a function used as a value.
    FnRef(String),
    EnumVariant {
        index: u32,
    },
    CallFn {
        symbol: String,
        args: Vec<Expr>,
    },
    CallExtern {
        name: String,
        args: Vec<Expr>,
    },
    CallIndirect {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    CallBuiltin {
        builtin: Builtin,
        args: Vec<Expr>,
    },
    /// Field access with the index of the field in its struct.
    FieldIdx {
        obj: Box<Expr>,
        struct_name: String,
        index: u32,
    },
    /// Struct literal with fields reordered to declaration order.
    New {
        struct_name: String,
        fields: Vec<Expr>,
    },
}
