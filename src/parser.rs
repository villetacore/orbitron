//! Recursive-descent parser with Pratt-style binary expressions.

use crate::ast::*;
use crate::diag::{Diag, Span};
use crate::lexer::{StrPart, Tok, Token};
use crate::types::Type;

pub struct Parser {
    toks: Vec<Token>,
    pos: usize,
    /// Inside `if`/`while`/`match`/`for` heads a `Name {` is not a struct literal.
    no_struct: bool,
}

type PResult<T> = Result<T, Diag>;

pub fn parse_program(tokens: Vec<Token>) -> PResult<Vec<Item>> {
    let mut p = Parser {
        toks: tokens,
        pos: 0,
        no_struct: false,
    };
    let mut items = Vec::new();
    while !p.at(&Tok::Eof) {
        items.push(p.item()?);
    }
    Ok(items)
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::Int(n) => format!("число {n}"),
        Tok::Float(f) => format!("число {f}"),
        Tok::Str(_) => "строка".into(),
        Tok::Ident(s) => format!("'{s}'"),
        Tok::Eof => "конец файла".into(),
        other => format!("'{}'", tok_text(other)),
    }
}

pub fn tok_text(t: &Tok) -> &'static str {
    match t {
        Tok::Let => "let",
        Tok::Var => "var",
        Tok::Const => "const",
        Tok::Fn => "fn",
        Tok::Return => "return",
        Tok::If => "if",
        Tok::Else => "else",
        Tok::While => "while",
        Tok::For => "for",
        Tok::In => "in",
        Tok::Loop => "loop",
        Tok::Break => "break",
        Tok::Continue => "continue",
        Tok::Match => "match",
        Tok::Struct => "struct",
        Tok::Impl => "impl",
        Tok::Enum => "enum",
        Tok::Import => "import",
        Tok::Extern => "extern",
        Tok::Defer => "defer",
        Tok::True => "true",
        Tok::False => "false",
        Tok::SelfKw => "self",
        Tok::As => "as",
        Tok::LParen => "(",
        Tok::RParen => ")",
        Tok::LBrace => "{",
        Tok::RBrace => "}",
        Tok::LBracket => "[",
        Tok::RBracket => "]",
        Tok::Comma => ",",
        Tok::Semi => ";",
        Tok::Colon => ":",
        Tok::ColonColon => "::",
        Tok::Dot => ".",
        Tok::DotDot => "..",
        Tok::DotDotEq => "..=",
        Tok::Arrow => "->",
        Tok::FatArrow => "=>",
        Tok::Plus => "+",
        Tok::Minus => "-",
        Tok::Star => "*",
        Tok::StarStar => "**",
        Tok::Slash => "/",
        Tok::Percent => "%",
        Tok::Amp => "&",
        Tok::Pipe => "|",
        Tok::Caret => "^",
        Tok::Tilde => "~",
        Tok::Bang => "!",
        Tok::Shl => "<<",
        Tok::Shr => ">>",
        Tok::AndAnd => "&&",
        Tok::OrOr => "||",
        Tok::PipeGt => "|>",
        Tok::Assign => "=",
        Tok::EqEq => "==",
        Tok::Ne => "!=",
        Tok::Lt => "<",
        Tok::Le => "<=",
        Tok::Gt => ">",
        Tok::Ge => ">=",
        Tok::PlusEq => "+=",
        Tok::MinusEq => "-=",
        Tok::StarEq => "*=",
        Tok::SlashEq => "/=",
        Tok::PercentEq => "%=",
        Tok::AmpEq => "&=",
        Tok::PipeEq => "|=",
        Tok::CaretEq => "^=",
        Tok::ShlEq => "<<=",
        Tok::ShrEq => ">>=",
        Tok::Int(_) | Tok::Float(_) | Tok::Str(_) | Tok::Ident(_) | Tok::Eof => "",
    }
}

fn binop_of(t: &Tok) -> Option<BinOp> {
    Some(match t {
        Tok::Plus => BinOp::Add,
        Tok::Minus => BinOp::Sub,
        Tok::Star => BinOp::Mul,
        Tok::Slash => BinOp::Div,
        Tok::Percent => BinOp::Rem,
        Tok::StarStar => BinOp::Pow,
        Tok::Amp => BinOp::BitAnd,
        Tok::Pipe => BinOp::BitOr,
        Tok::Caret => BinOp::BitXor,
        Tok::Shl => BinOp::Shl,
        Tok::Shr => BinOp::Shr,
        Tok::EqEq => BinOp::Eq,
        Tok::Ne => BinOp::Ne,
        Tok::Lt => BinOp::Lt,
        Tok::Le => BinOp::Le,
        Tok::Gt => BinOp::Gt,
        Tok::Ge => BinOp::Ge,
        Tok::AndAnd => BinOp::And,
        Tok::OrOr => BinOp::Or,
        _ => return None,
    })
}

fn assign_op(t: &Tok) -> Option<Option<BinOp>> {
    Some(match t {
        Tok::Assign => None,
        Tok::PlusEq => Some(BinOp::Add),
        Tok::MinusEq => Some(BinOp::Sub),
        Tok::StarEq => Some(BinOp::Mul),
        Tok::SlashEq => Some(BinOp::Div),
        Tok::PercentEq => Some(BinOp::Rem),
        Tok::AmpEq => Some(BinOp::BitAnd),
        Tok::PipeEq => Some(BinOp::BitOr),
        Tok::CaretEq => Some(BinOp::BitXor),
        Tok::ShlEq => Some(BinOp::Shl),
        Tok::ShrEq => Some(BinOp::Shr),
        _ => return None,
    })
}

impl Parser {
    // ── Token helpers ────────────────────────────────────────────────────────

    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn peek_at(&self, n: usize) -> &Tok {
        let i = (self.pos + n).min(self.toks.len() - 1);
        &self.toks[i].tok
    }

    fn span(&self) -> Span {
        self.toks[self.pos].span
    }

    fn at(&self, t: &Tok) -> bool {
        self.peek() == t
    }

    fn bump(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.at(t) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, t: &Tok) -> PResult<Span> {
        if self.at(t) {
            Ok(self.bump().span)
        } else {
            Err(self.unexpected(&format!("'{}'", tok_text(t))))
        }
    }

    fn unexpected(&self, wanted: &str) -> Diag {
        Diag::new(
            self.span(),
            format!("ожидалось {wanted}, найдено {}", describe(self.peek())),
        )
    }

    fn ident(&mut self) -> PResult<(String, Span)> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                let sp = self.bump().span;
                Ok((s, sp))
            }
            _ => Err(self.unexpected("имя")),
        }
    }

    // ── Items ────────────────────────────────────────────────────────────────

    fn item(&mut self) -> PResult<Item> {
        let span = self.span();
        let kind =
            match self.peek() {
                Tok::Import => {
                    self.bump();
                    let path = match self.peek().clone() {
                        Tok::Str(parts) => match parts.as_slice() {
                            [StrPart::Lit(s)] => {
                                self.bump();
                                s.clone()
                            }
                            _ => {
                                return Err(Diag::new(
                                    self.span(),
                                    "путь импорта не может содержать интерполяцию",
                                ))
                            }
                        },
                        _ => return Err(self.unexpected("строка с путём модуля")),
                    };
                    self.expect(&Tok::Semi)?;
                    ItemKind::Import(path)
                }
                Tok::Const => ItemKind::Const(self.const_decl()?),
                Tok::Fn => ItemKind::Fn(self.fn_decl(false)?),
                Tok::Extern => {
                    self.bump();
                    self.expect(&Tok::Fn)?;
                    let (name, _) = self.ident()?;
                    self.expect(&Tok::LParen)?;
                    let (has_self, params) = self.params(false)?;
                    debug_assert!(!has_self);
                    let ret = if self.eat(&Tok::Arrow) {
                        Some(self.type_expr()?)
                    } else {
                        None
                    };
                    self.expect(&Tok::Semi)?;
                    ItemKind::Extern(ExternDecl { name, params, ret })
                }
                Tok::Struct => {
                    self.bump();
                    let (name, _) = self.ident()?;
                    self.expect(&Tok::LBrace)?;
                    let mut fields = Vec::new();
                    while !self.at(&Tok::RBrace) {
                        let (fname, fspan) = self.ident()?;
                        self.expect(&Tok::Colon)?;
                        let ty = self.type_expr()?;
                        fields.push(Field {
                            name: fname,
                            ty,
                            span: fspan,
                        });
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                    self.expect(&Tok::RBrace)?;
                    ItemKind::Struct(StructDecl { name, fields })
                }
                Tok::Enum => {
                    self.bump();
                    let (name, _) = self.ident()?;
                    self.expect(&Tok::LBrace)?;
                    let mut variants = Vec::new();
                    while !self.at(&Tok::RBrace) {
                        variants.push(self.ident()?);
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                    self.expect(&Tok::RBrace)?;
                    ItemKind::Enum(EnumDecl { name, variants })
                }
                Tok::Impl => {
                    self.bump();
                    let (type_name, _) = self.ident()?;
                    self.expect(&Tok::LBrace)?;
                    let mut methods = Vec::new();
                    while !self.at(&Tok::RBrace) {
                        if !self.at(&Tok::Fn) {
                            return Err(
                                self.unexpected("'fn' (в блоке impl допускаются только функции)")
                            );
                        }
                        methods.push(self.fn_decl(true)?);
                    }
                    self.expect(&Tok::RBrace)?;
                    ItemKind::Impl(ImplDecl { type_name, methods })
                }
                Tok::Let | Tok::Var => {
                    return Err(Diag::new(span, "переменные нельзя объявлять вне функций")
                        .with_note(
                            "для глобальных значений используйте 'const ИМЯ: тип = значение;'",
                        ));
                }
                _ => {
                    return Err(self
                        .unexpected("объявление (fn, struct, enum, impl, const, import, extern)"))
                }
            };
        Ok(Item { kind, span })
    }

    fn const_decl(&mut self) -> PResult<ConstDecl> {
        self.expect(&Tok::Const)?;
        let (name, _) = self.ident()?;
        let ty = if self.eat(&Tok::Colon) {
            Some(self.type_expr()?)
        } else {
            None
        };
        self.expect(&Tok::Assign)?;
        let value = self.expr()?;
        self.expect(&Tok::Semi)?;
        Ok(ConstDecl { name, ty, value })
    }

    fn fn_decl(&mut self, allow_self: bool) -> PResult<FnDecl> {
        let span = self.expect(&Tok::Fn)?;
        let (name, _) = self.ident()?;
        self.expect(&Tok::LParen)?;
        let (has_self, params) = self.params(allow_self)?;
        let ret = if self.eat(&Tok::Arrow) {
            Some(self.type_expr()?)
        } else {
            None
        };
        let body = self.block()?;
        Ok(FnDecl {
            name,
            has_self,
            params,
            ret,
            body,
            span,
            symbol: String::new(),
            locals: Vec::new(),
            ret_ty: Type::Unknown,
        })
    }

    /// Parses parameters after `(` up to and including `)`.
    fn params(&mut self, allow_self: bool) -> PResult<(bool, Vec<Param>)> {
        let mut has_self = false;
        let mut params = Vec::new();
        if self.at(&Tok::SelfKw) {
            if !allow_self {
                return Err(Diag::new(
                    self.span(),
                    "'self' допускается только в методах внутри impl",
                ));
            }
            self.bump();
            has_self = true;
            if !self.eat(&Tok::Comma) {
                self.expect(&Tok::RParen)?;
                return Ok((has_self, params));
            }
        }
        while !self.at(&Tok::RParen) {
            let (name, span) = self.ident()?;
            self.expect(&Tok::Colon)?;
            let ty = self.type_expr()?;
            params.push(Param { name, ty, span });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen)?;
        Ok((has_self, params))
    }

    fn type_expr(&mut self) -> PResult<TypeExpr> {
        let span = self.span();
        match self.peek().clone() {
            Tok::Ident(name) => {
                self.bump();
                Ok(TypeExpr::Named(name, span))
            }
            Tok::LBracket => {
                self.bump();
                let inner = self.type_expr()?;
                self.expect(&Tok::RBracket)?;
                Ok(TypeExpr::Array(Box::new(inner), span))
            }
            Tok::Fn => {
                self.bump();
                self.expect(&Tok::LParen)?;
                let mut ps = Vec::new();
                while !self.at(&Tok::RParen) {
                    ps.push(self.type_expr()?);
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                self.expect(&Tok::RParen)?;
                let ret = if self.eat(&Tok::Arrow) {
                    Some(Box::new(self.type_expr()?))
                } else {
                    None
                };
                Ok(TypeExpr::Fn(ps, ret, span))
            }
            _ => Err(self.unexpected("тип")),
        }
    }

    // ── Blocks and statements ────────────────────────────────────────────────

    fn block(&mut self) -> PResult<Block> {
        let span = self.expect(&Tok::LBrace)?;
        let saved = std::mem::replace(&mut self.no_struct, false);
        let mut stmts = Vec::new();
        let mut tail = None;
        loop {
            if self.at(&Tok::RBrace) {
                break;
            }
            if self.eat(&Tok::Semi) {
                continue;
            }
            let sspan = self.span();
            match self.peek() {
                Tok::Let | Tok::Var => {
                    let mutable = self.bump().tok == Tok::Var;
                    let (name, _) = self.ident()?;
                    let ty = if self.eat(&Tok::Colon) {
                        Some(self.type_expr()?)
                    } else {
                        None
                    };
                    if !self.at(&Tok::Assign) {
                        return Err(
                            self.unexpected("'=' (переменная должна быть инициализирована)")
                        );
                    }
                    self.bump();
                    let init = self.expr()?;
                    self.expect(&Tok::Semi)?;
                    stmts.push(Stmt {
                        kind: StmtKind::Let {
                            name,
                            mutable,
                            ty,
                            init,
                            local: 0,
                        },
                        span: sspan,
                    });
                }
                Tok::Defer => {
                    self.bump();
                    let e = self.expr()?;
                    if !e.is_block_like() || self.at(&Tok::Semi) {
                        self.expect(&Tok::Semi)?;
                    }
                    stmts.push(Stmt {
                        kind: StmtKind::Defer(e),
                        span: sspan,
                    });
                }
                Tok::Fn
                | Tok::Struct
                | Tok::Enum
                | Tok::Impl
                | Tok::Const
                | Tok::Import
                | Tok::Extern
                    if !matches!(self.peek_at(1), Tok::LParen) =>
                {
                    return Err(Diag::new(
                        sspan,
                        "объявления допускаются только на верхнем уровне файла",
                    ));
                }
                _ => {
                    let block_start = matches!(
                        self.peek(),
                        Tok::If | Tok::Match | Tok::While | Tok::For | Tok::Loop | Tok::LBrace
                    );
                    let e = if block_start {
                        self.primary()?
                    } else {
                        self.expr()?
                    };
                    if let Some(op) = assign_op(self.peek()) {
                        self.bump();
                        let value = self.expr()?;
                        self.expect(&Tok::Semi)?;
                        stmts.push(Stmt {
                            kind: StmtKind::Assign {
                                target: e,
                                op,
                                value,
                            },
                            span: sspan,
                        });
                    } else if self.eat(&Tok::Semi) {
                        stmts.push(Stmt {
                            kind: StmtKind::Expr {
                                expr: e,
                                semi: true,
                            },
                            span: sspan,
                        });
                    } else if self.at(&Tok::RBrace) {
                        tail = Some(Box::new(e));
                        break;
                    } else if e.is_block_like() {
                        stmts.push(Stmt {
                            kind: StmtKind::Expr {
                                expr: e,
                                semi: false,
                            },
                            span: sspan,
                        });
                    } else {
                        return Err(self.unexpected("';'"));
                    }
                }
            }
        }
        let end_line = self.span().line;
        self.expect(&Tok::RBrace)?;
        self.no_struct = saved;
        Ok(Block {
            stmts,
            tail,
            span,
            end_line,
        })
    }

    // ── Expressions ──────────────────────────────────────────────────────────

    pub fn expr(&mut self) -> PResult<Expr> {
        self.binary(0)
    }

    fn expr_no_struct(&mut self) -> PResult<Expr> {
        let saved = std::mem::replace(&mut self.no_struct, true);
        let r = self.expr();
        self.no_struct = saved;
        r
    }

    fn binary(&mut self, min_prec: u8) -> PResult<Expr> {
        let mut lhs = self.unary()?;
        loop {
            let span = self.span();
            if self.at(&Tok::PipeGt) {
                if PIPE_PREC < min_prec {
                    break;
                }
                self.bump();
                let rhs = self.binary(PIPE_PREC + 1)?;
                lhs = Expr::new(ExprKind::Pipe(Box::new(lhs), Box::new(rhs)), span);
                continue;
            }
            if self.at(&Tok::As) {
                if UNARY_PREC < min_prec {
                    break;
                }
                self.bump();
                let ty = self.type_expr()?;
                lhs = Expr::new(ExprKind::Cast(Box::new(lhs), ty), span);
                continue;
            }
            let op = match binop_of(self.peek()) {
                Some(op) => op,
                None => break,
            };
            let prec = op.precedence();
            if prec < min_prec {
                break;
            }
            self.bump();
            // `**` is right-associative, everything else left-associative.
            let next_min = if op == BinOp::Pow { prec } else { prec + 1 };
            let rhs = self.binary(next_min)?;
            lhs = Expr::new(ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)), span);
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> PResult<Expr> {
        let span = self.span();
        let op = match self.peek() {
            Tok::Minus => UnOp::Neg,
            Tok::Bang => UnOp::Not,
            Tok::Tilde => UnOp::BitNot,
            _ => return self.postfix(),
        };
        self.bump();
        // Operand binds tighter than everything except `**`: `-x ** 2 == -(x ** 2)`.
        let operand = self.binary(BinOp::Pow.precedence())?;
        Ok(Expr::new(ExprKind::Unary(op, Box::new(operand)), span))
    }

    fn postfix(&mut self) -> PResult<Expr> {
        let mut e = self.primary()?;
        loop {
            let span = self.span();
            match self.peek() {
                Tok::LParen => {
                    self.bump();
                    let args = self.args(&Tok::RParen)?;
                    let start = e.span;
                    e = Expr::new(
                        ExprKind::Call {
                            callee: Box::new(e),
                            args,
                        },
                        start,
                    );
                }
                Tok::Dot => {
                    self.bump();
                    let (name, _) = self.ident()?;
                    if self.at(&Tok::LParen) {
                        self.bump();
                        let args = self.args(&Tok::RParen)?;
                        e = Expr::new(
                            ExprKind::MethodCall {
                                recv: Box::new(e),
                                method: name,
                                args,
                            },
                            span,
                        );
                    } else {
                        e = Expr::new(
                            ExprKind::Field {
                                obj: Box::new(e),
                                name,
                            },
                            span,
                        );
                    }
                }
                Tok::LBracket => {
                    self.bump();
                    let saved = std::mem::replace(&mut self.no_struct, false);
                    let idx = self.expr()?;
                    self.no_struct = saved;
                    self.expect(&Tok::RBracket)?;
                    e = Expr::new(
                        ExprKind::Index {
                            obj: Box::new(e),
                            idx: Box::new(idx),
                        },
                        span,
                    );
                }
                _ => break,
            }
        }
        Ok(e)
    }

    /// Comma-separated expressions up to and including `close`.
    fn args(&mut self, close: &Tok) -> PResult<Vec<Expr>> {
        let saved = std::mem::replace(&mut self.no_struct, false);
        let mut out = Vec::new();
        while !self.at(close) {
            out.push(self.expr()?);
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(close)?;
        self.no_struct = saved;
        Ok(out)
    }

    fn primary(&mut self) -> PResult<Expr> {
        let span = self.span();
        let kind = match self.peek().clone() {
            Tok::Int(n) => {
                self.bump();
                ExprKind::Int(n)
            }
            Tok::Float(f) => {
                self.bump();
                ExprKind::Float(f)
            }
            Tok::True => {
                self.bump();
                ExprKind::Bool(true)
            }
            Tok::False => {
                self.bump();
                ExprKind::Bool(false)
            }
            Tok::Str(parts) => {
                self.bump();
                self.string(parts)?
            }
            Tok::SelfKw => {
                self.bump();
                ExprKind::SelfRef
            }
            Tok::Ident(name) => {
                self.bump();
                if self.eat(&Tok::ColonColon) {
                    let (member, _) = self.ident()?;
                    ExprKind::Path(name, member)
                } else if self.at(&Tok::LBrace) && !self.no_struct {
                    self.struct_lit(name)?
                } else {
                    ExprKind::Ident(name)
                }
            }
            Tok::LParen => {
                self.bump();
                let saved = std::mem::replace(&mut self.no_struct, false);
                let inner = self.expr()?;
                self.no_struct = saved;
                self.expect(&Tok::RParen)?;
                ExprKind::Paren(Box::new(inner))
            }
            Tok::LBracket => {
                self.bump();
                let saved = std::mem::replace(&mut self.no_struct, false);
                let kind = if self.eat(&Tok::RBracket) {
                    ExprKind::ArrayLit(Vec::new())
                } else {
                    let first = self.expr()?;
                    if self.eat(&Tok::Semi) {
                        let count = self.expr()?;
                        self.expect(&Tok::RBracket)?;
                        ExprKind::ArrayRepeat {
                            value: Box::new(first),
                            count: Box::new(count),
                        }
                    } else {
                        let mut items = vec![first];
                        if self.eat(&Tok::Comma) {
                            items.extend(self.args(&Tok::RBracket)?);
                        } else {
                            self.expect(&Tok::RBracket)?;
                        }
                        ExprKind::ArrayLit(items)
                    }
                };
                self.no_struct = saved;
                kind
            }
            Tok::Fn => {
                self.bump();
                self.expect(&Tok::LParen)?;
                let (_, params) = self.params(false)?;
                let ret = if self.eat(&Tok::Arrow) {
                    Some(self.type_expr()?)
                } else {
                    None
                };
                let body = self.block()?;
                ExprKind::Lambda { params, ret, body }
            }
            Tok::If => return self.if_expr(),
            Tok::Match => {
                self.bump();
                let scrut = self.expr_no_struct()?;
                self.expect(&Tok::LBrace)?;
                let saved = std::mem::replace(&mut self.no_struct, false);
                let mut arms = Vec::new();
                while !self.at(&Tok::RBrace) {
                    let aspan = self.span();
                    let mut pats = vec![self.pattern()?];
                    while self.eat(&Tok::Pipe) {
                        pats.push(self.pattern()?);
                    }
                    self.expect(&Tok::FatArrow)?;
                    let body = self.expr()?;
                    let block_body = matches!(body.kind, ExprKind::Block(_));
                    arms.push(MatchArm {
                        pats,
                        body,
                        span: aspan,
                    });
                    if !self.eat(&Tok::Comma) && !block_body && !self.at(&Tok::RBrace) {
                        return Err(self.unexpected("',' после ветки match"));
                    }
                }
                self.expect(&Tok::RBrace)?;
                self.no_struct = saved;
                ExprKind::Match {
                    scrut: Box::new(scrut),
                    arms,
                }
            }
            Tok::LBrace => ExprKind::Block(self.block()?),
            Tok::While => {
                self.bump();
                let cond = self.expr_no_struct()?;
                let body = self.block()?;
                ExprKind::While {
                    cond: Box::new(cond),
                    body,
                }
            }
            Tok::For => {
                self.bump();
                let (var, _) = self.ident()?;
                self.expect(&Tok::In)?;
                let start = self.expr_no_struct()?;
                let iter = if self.at(&Tok::DotDot) || self.at(&Tok::DotDotEq) {
                    let inclusive = self.bump().tok == Tok::DotDotEq;
                    let end = self.expr_no_struct()?;
                    ForIter::Range {
                        start: Box::new(start),
                        end: Box::new(end),
                        inclusive,
                    }
                } else {
                    ForIter::Each(Box::new(start))
                };
                let body = self.block()?;
                ExprKind::For {
                    var,
                    local: 0,
                    iter,
                    body,
                }
            }
            Tok::Loop => {
                self.bump();
                ExprKind::Loop(self.block()?)
            }
            Tok::Break => {
                self.bump();
                ExprKind::Break
            }
            Tok::Continue => {
                self.bump();
                ExprKind::Continue
            }
            Tok::Return => {
                self.bump();
                let value = match self.peek() {
                    Tok::Semi | Tok::RBrace | Tok::Comma | Tok::RParen => None,
                    _ => Some(Box::new(self.expr()?)),
                };
                ExprKind::Return(value)
            }
            Tok::DotDot | Tok::DotDotEq => {
                return Err(Diag::new(
                    span,
                    "диапазоны допускаются только в 'for' и в образцах 'match'",
                ));
            }
            _ => return Err(self.unexpected("выражение")),
        };
        Ok(Expr::new(kind, span))
    }

    fn if_expr(&mut self) -> PResult<Expr> {
        let span = self.expect(&Tok::If)?;
        let cond = self.expr_no_struct()?;
        let then = self.block()?;
        let els = if self.eat(&Tok::Else) {
            if self.at(&Tok::If) {
                Some(Box::new(self.if_expr()?))
            } else {
                let bspan = self.span();
                let b = self.block()?;
                Some(Box::new(Expr::new(ExprKind::Block(b), bspan)))
            }
        } else {
            None
        };
        Ok(Expr::new(
            ExprKind::If {
                cond: Box::new(cond),
                then,
                els,
            },
            span,
        ))
    }

    fn struct_lit(&mut self, name: String) -> PResult<ExprKind> {
        self.expect(&Tok::LBrace)?;
        let mut fields = Vec::new();
        while !self.at(&Tok::RBrace) {
            let (fname, fspan) = self.ident()?;
            let value = if self.eat(&Tok::Colon) {
                self.expr()?
            } else {
                Expr::new(ExprKind::Ident(fname.clone()), fspan)
            };
            fields.push((fname, value));
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RBrace)?;
        Ok(ExprKind::StructLit { name, fields })
    }

    fn string(&mut self, parts: Vec<StrPart>) -> PResult<ExprKind> {
        if let [StrPart::Lit(s)] = parts.as_slice() {
            return Ok(ExprKind::Str(s.clone()));
        }
        let mut out = Vec::new();
        for part in parts {
            match part {
                StrPart::Lit(s) => out.push(InterpPart::Lit(s)),
                StrPart::Expr(toks) => {
                    let mut sub = Parser {
                        toks,
                        pos: 0,
                        no_struct: false,
                    };
                    let e = sub.expr()?;
                    if !sub.at(&Tok::Eof) {
                        return Err(sub.unexpected("'}' в конце интерполяции"));
                    }
                    out.push(InterpPart::Expr(e));
                }
            }
        }
        Ok(ExprKind::Interp(out))
    }

    fn pattern(&mut self) -> PResult<Pat> {
        let span = self.span();
        match self.peek().clone() {
            Tok::Ident(name) if name == "_" => {
                self.bump();
                Ok(Pat::Wildcard(span))
            }
            Tok::Ident(name) => {
                self.bump();
                if !self.eat(&Tok::ColonColon) {
                    return Err(Diag::new(span, format!("неизвестный образец '{name}'"))
                        .with_note("варианты перечислений записываются как Enum::Variant"));
                }
                let (variant, _) = self.ident()?;
                Ok(Pat::Variant(name, variant, span, 0))
            }
            Tok::Str(parts) => match parts.as_slice() {
                [StrPart::Lit(s)] => {
                    self.bump();
                    Ok(Pat::Str(s.clone(), span))
                }
                _ => Err(Diag::new(
                    span,
                    "образец-строка не может содержать интерполяцию",
                )),
            },
            Tok::True => {
                self.bump();
                Ok(Pat::Bool(true, span))
            }
            Tok::False => {
                self.bump();
                Ok(Pat::Bool(false, span))
            }
            Tok::Int(_) | Tok::Minus => {
                let lo = self.pat_int()?;
                if self.at(&Tok::DotDot) || self.at(&Tok::DotDotEq) {
                    let inclusive = self.bump().tok == Tok::DotDotEq;
                    let hi = self.pat_int()?;
                    Ok(Pat::Range(lo, hi, inclusive, span))
                } else {
                    Ok(Pat::Int(lo, span))
                }
            }
            _ => Err(self.unexpected("образец (число, строка, true/false, Enum::Variant или _)")),
        }
    }

    fn pat_int(&mut self) -> PResult<i64> {
        let neg = self.eat(&Tok::Minus);
        match self.peek().clone() {
            Tok::Int(n) => {
                self.bump();
                Ok(if neg { n.wrapping_neg() } else { n })
            }
            _ => Err(self.unexpected("целое число")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;

    fn parse(src: &str) -> Vec<Item> {
        parse_program(tokenize(src, 0).unwrap().tokens).unwrap()
    }

    fn parse_err(src: &str) -> String {
        parse_program(tokenize(src, 0).unwrap().tokens)
            .unwrap_err()
            .msg
    }

    #[test]
    fn precedence() {
        let items = parse("fn f() -> int { 1 + 2 * 3 ** 2 }");
        let ItemKind::Fn(f) = &items[0].kind else {
            panic!()
        };
        let tail = f.body.tail.as_ref().unwrap();
        let ExprKind::Binary(BinOp::Add, _, rhs) = &tail.kind else {
            panic!("{tail:?}")
        };
        assert!(matches!(rhs.kind, ExprKind::Binary(BinOp::Mul, _, _)));
    }

    #[test]
    fn struct_literal_not_in_condition() {
        parse("struct P { x: int } fn f(p: int) { if p == 1 { } let q = P { x: 1 }; }");
    }

    #[test]
    fn statements_need_semicolons() {
        assert!(parse_err("fn f() { let x = 1 let y = 2; }").contains("';'"));
    }

    #[test]
    fn no_globals() {
        assert!(parse_err("var x = 1;").contains("вне функций"));
    }
}
