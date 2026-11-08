//! Semantic analysis: name resolution, type checking and lowering of
//! convenience syntax (pipes, lambdas, constants) into resolved AST forms.

use std::collections::HashMap;

use crate::ast::*;
use crate::diag::{Diag, Span};
use crate::types::Type;

// ── Output ────────────────────────────────────────────────────────────────────

/// A fully checked program, ready for code generation.
pub struct Module {
    pub fns: Vec<FnDecl>,
    pub structs: Vec<StructInfo>,
    pub enums: Vec<EnumInfo>,
    pub externs: Vec<ExternInfo>,
}

#[derive(Debug, Clone)]
pub struct StructInfo {
    pub name: String,
    pub fields: Vec<(String, Type)>,
}

#[derive(Debug, Clone)]
pub struct EnumInfo {
    pub name: String,
    pub variants: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ExternInfo {
    pub name: String,
    pub params: Vec<Type>,
    pub ret: Type,
}

impl Module {
    pub fn struct_info(&self, name: &str) -> &StructInfo {
        self.structs
            .iter()
            .find(|s| s.name == name)
            .expect("unknown struct")
    }
}

// ── Internal tables ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct FnSig {
    params: Vec<Type>,
    ret: Type,
    has_self: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum ConstVal {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
}

impl ConstVal {
    fn ty(&self) -> Type {
        match self {
            ConstVal::Int(_) => Type::Int,
            ConstVal::Float(_) => Type::Float,
            ConstVal::Bool(_) => Type::Bool,
            ConstVal::Str(_) => Type::Str,
        }
    }

    fn to_expr(&self, span: Span) -> Expr {
        let kind = match self {
            ConstVal::Int(n) => ExprKind::Int(*n),
            ConstVal::Float(f) => ExprKind::Float(*f),
            ConstVal::Bool(b) => ExprKind::Bool(*b),
            ConstVal::Str(s) => ExprKind::Str(s.clone()),
        };
        Expr {
            kind,
            span,
            ty: self.ty(),
        }
    }
}

#[allow(clippy::large_enum_variant)]
enum ConstState {
    Pending(ConstDecl, Span),
    InProgress,
    Done(ConstVal),
    Failed,
}

/// What the context expects from an expression.
#[derive(Clone, Copy)]
enum Expect<'a> {
    Any,
    Type(&'a Type),
    /// The value is thrown away (expression statement).
    Discard,
}

impl<'a> Expect<'a> {
    fn ty(&self) -> Option<&'a Type> {
        match self {
            Expect::Type(t) => Some(t),
            _ => None,
        }
    }
}

struct LoopCtx {
    has_break: bool,
}

/// Per-function checking state.
struct FnCtx {
    locals: Vec<LocalInfo>,
    scopes: Vec<HashMap<String, LocalId>>,
    ret: Type,
    loops: Vec<LoopCtx>,
    in_defer: usize,
    self_type: Option<Type>,
}

pub struct Checker {
    diags: Vec<Diag>,
    structs: Vec<StructInfo>,
    struct_idx: HashMap<String, usize>,
    enums: HashMap<String, Vec<String>>,
    enum_order: Vec<String>,
    fns: HashMap<String, FnSig>,
    externs: HashMap<String, FnSig>,
    consts: HashMap<String, ConstState>,
    fn_stack: Vec<FnCtx>,
    lambdas: Vec<FnDecl>,
    lambda_counter: usize,
}

pub fn check(items: Vec<Item>, require_main: bool) -> Result<Module, Vec<Diag>> {
    let mut c = Checker {
        diags: Vec::new(),
        structs: Vec::new(),
        struct_idx: HashMap::new(),
        enums: HashMap::new(),
        enum_order: Vec::new(),
        fns: HashMap::new(),
        externs: HashMap::new(),
        consts: HashMap::new(),
        fn_stack: Vec::new(),
        lambdas: Vec::new(),
        lambda_counter: 0,
    };
    let module = c.run(items, require_main);
    if c.diags.is_empty() {
        Ok(module)
    } else {
        let mut d = c.diags;
        d.sort_by_key(|d| (d.span.file, d.span.line, d.span.col));
        d.dedup_by(|a, b| a.span == b.span && a.msg == b.msg);
        Err(d)
    }
}

fn is_builtin_type(name: &str) -> bool {
    matches!(name, "int" | "float" | "bool" | "str" | "void")
}

impl Checker {
    fn err(&mut self, span: Span, msg: impl Into<String>) {
        self.diags.push(Diag::new(span, msg));
    }

    fn err_note(&mut self, span: Span, msg: impl Into<String>, note: impl Into<String>) {
        self.diags.push(Diag::new(span, msg).with_note(note));
    }

    // ── Collection ───────────────────────────────────────────────────────────

    fn run(&mut self, items: Vec<Item>, require_main: bool) -> Module {
        let mut fns: Vec<FnDecl> = Vec::new();
        let mut externs_out = Vec::new();
        let mut structs_src = Vec::new();
        let mut impls = Vec::new();

        // Pass 1: names of types, constants.
        for item in items {
            let span = item.span;
            match item.kind {
                ItemKind::Import(_) => {}
                ItemKind::Struct(s) => {
                    if self.type_exists(&s.name) || is_builtin_type(&s.name) {
                        self.err(span, format!("тип '{}' уже объявлен", s.name));
                        continue;
                    }
                    self.struct_idx.insert(s.name.clone(), self.structs.len());
                    self.structs.push(StructInfo {
                        name: s.name.clone(),
                        fields: Vec::new(),
                    });
                    structs_src.push(s);
                }
                ItemKind::Enum(e) => {
                    if self.type_exists(&e.name) || is_builtin_type(&e.name) {
                        self.err(span, format!("тип '{}' уже объявлен", e.name));
                        continue;
                    }
                    let mut names: Vec<String> = Vec::new();
                    for (v, vspan) in &e.variants {
                        if names.contains(v) {
                            self.err(
                                *vspan,
                                format!("вариант '{}' повторяется в enum {}", v, e.name),
                            );
                        }
                        names.push(v.clone());
                    }
                    if names.is_empty() {
                        self.err(span, format!("enum {} не содержит вариантов", e.name));
                    }
                    self.enum_order.push(e.name.clone());
                    self.enums.insert(e.name, names);
                }
                ItemKind::Const(cd) => {
                    if self.consts.contains_key(&cd.name) {
                        self.err(span, format!("константа '{}' уже объявлена", cd.name));
                        continue;
                    }
                    self.consts
                        .insert(cd.name.clone(), ConstState::Pending(cd, span));
                }
                ItemKind::Fn(f) => fns.push(f),
                ItemKind::Impl(i) => impls.push((i, span)),
                ItemKind::Extern(e) => externs_out.push((e, span)),
            }
        }

        // Pass 2: struct fields (types may now refer to any struct/enum).
        for s in &structs_src {
            let mut fields: Vec<(String, Type)> = Vec::new();
            for f in &s.fields {
                if fields.iter().any(|(n, _)| n == &f.name) {
                    self.err(
                        f.span,
                        format!("поле '{}' повторяется в struct {}", f.name, s.name),
                    );
                    continue;
                }
                let ty = self.resolve_type(&f.ty);
                fields.push((f.name.clone(), ty));
            }
            let idx = self.struct_idx[&s.name];
            self.structs[idx].fields = fields;
        }

        // Pass 3: function signatures.
        for f in fns.iter_mut() {
            f.symbol = f.name.clone();
            self.declare_fn(f, None);
        }
        for (imp, span) in impls {
            let self_ty = if self.struct_idx.contains_key(&imp.type_name) {
                Type::Struct(imp.type_name.clone())
            } else if self.enums.contains_key(&imp.type_name) {
                Type::Enum(imp.type_name.clone())
            } else {
                self.err(
                    span,
                    format!("impl для неизвестного типа '{}'", imp.type_name),
                );
                continue;
            };
            for mut m in imp.methods {
                m.symbol = format!("{}::{}", imp.type_name, m.name);
                self.declare_fn(&mut m, Some(&self_ty));
                fns.push(m);
            }
        }
        let mut externs = Vec::new();
        for (e, span) in externs_out {
            if self.fns.contains_key(&e.name) || self.externs.contains_key(&e.name) {
                self.err(span, format!("функция '{}' уже объявлена", e.name));
                continue;
            }
            let params: Vec<Type> = e.params.iter().map(|p| self.resolve_type(&p.ty)).collect();
            let ret = e
                .ret
                .as_ref()
                .map(|t| self.resolve_type(t))
                .unwrap_or(Type::Void);
            for (p, t) in e.params.iter().zip(&params) {
                if !matches!(
                    t,
                    Type::Int | Type::Float | Type::Bool | Type::Str | Type::Error
                ) {
                    self.err(p.span, format!("тип '{t}' нельзя передать во внешнюю функцию (допустимы int, float, bool, str)"));
                }
            }
            if !matches!(
                ret,
                Type::Int | Type::Float | Type::Bool | Type::Str | Type::Void | Type::Error
            ) {
                self.err(span, format!("внешняя функция не может возвращать '{ret}'"));
            }
            self.externs.insert(
                e.name.clone(),
                FnSig {
                    params: params.clone(),
                    ret: ret.clone(),
                    has_self: false,
                },
            );
            externs.push(ExternInfo {
                name: e.name,
                params,
                ret,
            });
        }

        // Pass 4: constants.
        let names: Vec<String> = self.consts.keys().cloned().collect();
        for n in names {
            self.const_value(&n, Span::default());
        }

        if require_main {
            match self.fns.get("main") {
                None => self.err(Span::default(), "в программе нет функции 'fn main()'"),
                Some(sig) => {
                    let ok = sig.params.is_empty() && matches!(sig.ret, Type::Void | Type::Int);
                    if !ok {
                        let span = fns
                            .iter()
                            .find(|f| f.symbol == "main")
                            .map(|f| f.span)
                            .unwrap_or_default();
                        self.err(
                            span,
                            "функция main должна иметь вид 'fn main()' или 'fn main() -> int'",
                        );
                    }
                }
            }
        }

        // Pass 5: bodies.
        for f in fns.iter_mut() {
            self.check_fn(f);
        }
        let mut lambdas = std::mem::take(&mut self.lambdas);
        fns.append(&mut lambdas);

        let enums = self
            .enum_order
            .iter()
            .map(|n| EnumInfo {
                name: n.clone(),
                variants: self.enums[n].clone(),
            })
            .collect();
        Module {
            fns,
            structs: self.structs.clone(),
            enums,
            externs,
        }
    }

    fn type_exists(&self, name: &str) -> bool {
        self.struct_idx.contains_key(name) || self.enums.contains_key(name)
    }

    fn declare_fn(&mut self, f: &mut FnDecl, self_ty: Option<&Type>) {
        if self.fns.contains_key(&f.symbol)
            || (self_ty.is_none() && self.externs.contains_key(&f.symbol))
        {
            self.err(f.span, format!("функция '{}' уже объявлена", f.symbol));
            return;
        }
        let mut params = Vec::new();
        if f.has_self {
            params.push(self_ty.cloned().unwrap_or(Type::Error));
        }
        for p in &f.params {
            params.push(self.resolve_type(&p.ty));
        }
        let ret = f
            .ret
            .as_ref()
            .map(|t| self.resolve_type(t))
            .unwrap_or(Type::Void);
        f.ret_ty = ret.clone();
        self.fns.insert(
            f.symbol.clone(),
            FnSig {
                params,
                ret,
                has_self: f.has_self,
            },
        );
    }

    fn resolve_type(&mut self, t: &TypeExpr) -> Type {
        match t {
            TypeExpr::Named(n, span) => match n.as_str() {
                "int" => Type::Int,
                "float" => Type::Float,
                "bool" => Type::Bool,
                "str" => Type::Str,
                "void" => {
                    self.err_note(
                        *span,
                        "тип 'void' нельзя указывать явно",
                        "чтобы функция ничего не возвращала, просто не пишите '-> тип'",
                    );
                    Type::Error
                }
                "i64" | "i32" | "f64" | "f32" | "string" | "String" => {
                    let hint = match n.as_str() {
                        "i64" | "i32" => "int",
                        "f64" | "f32" => "float",
                        _ => "str",
                    };
                    self.err_note(
                        *span,
                        format!("неизвестный тип '{n}'"),
                        format!("используйте '{hint}'"),
                    );
                    Type::Error
                }
                _ if self.struct_idx.contains_key(n) => Type::Struct(n.clone()),
                _ if self.enums.contains_key(n) => Type::Enum(n.clone()),
                _ => {
                    self.err(*span, format!("неизвестный тип '{n}'"));
                    Type::Error
                }
            },
            TypeExpr::Array(inner, _) => Type::Array(Box::new(self.resolve_type(inner))),
            TypeExpr::Fn(ps, r, _) => {
                let ps = ps.iter().map(|p| self.resolve_type(p)).collect();
                let r = r
                    .as_ref()
                    .map(|r| self.resolve_type(r))
                    .unwrap_or(Type::Void);
                Type::Fn(ps, Box::new(r))
            }
        }
    }

    // ── Constants ────────────────────────────────────────────────────────────

    fn const_value(&mut self, name: &str, use_span: Span) -> Option<ConstVal> {
        let state = self.consts.get_mut(name)?;
        match std::mem::replace(state, ConstState::InProgress) {
            ConstState::Done(v) => {
                *self.consts.get_mut(name).unwrap() = ConstState::Done(v.clone());
                Some(v)
            }
            ConstState::Failed => {
                *self.consts.get_mut(name).unwrap() = ConstState::Failed;
                None
            }
            ConstState::InProgress => {
                self.err(
                    use_span,
                    format!("константа '{name}' определена через саму себя"),
                );
                *self.consts.get_mut(name).unwrap() = ConstState::Failed;
                None
            }
            ConstState::Pending(cd, span) => {
                let result = self.eval_const(&cd.value);
                let result = match (result, &cd.ty) {
                    (Ok(v), Some(t)) => {
                        let want = self.resolve_type(t);
                        if want == v.ty() || want.is_error() {
                            Ok(v)
                        } else {
                            Err(Diag::new(
                                span,
                                format!(
                                    "константа '{}' объявлена как {}, но её значение имеет тип {}",
                                    cd.name,
                                    want,
                                    v.ty()
                                ),
                            ))
                        }
                    }
                    (r, _) => r,
                };
                let st = match result {
                    Ok(v) => ConstState::Done(v),
                    Err(d) => {
                        self.diags.push(d);
                        ConstState::Failed
                    }
                };
                let out = if let ConstState::Done(v) = &st {
                    Some(v.clone())
                } else {
                    None
                };
                *self.consts.get_mut(name).unwrap() = st;
                out
            }
        }
    }

    fn eval_const(&mut self, e: &Expr) -> Result<ConstVal, Diag> {
        use ConstVal as C;
        let bad = |span: Span| {
            Diag::new(span, "значение константы должно вычисляться при компиляции").with_note(
                "допустимы литералы, другие константы, арифметика, сравнения и приведения типов",
            )
        };
        Ok(match &e.kind {
            ExprKind::Int(n) => C::Int(*n),
            ExprKind::Float(f) => C::Float(*f),
            ExprKind::Bool(b) => C::Bool(*b),
            ExprKind::Str(s) => C::Str(s.clone()),
            ExprKind::Paren(inner) => self.eval_const(inner)?,
            ExprKind::Ident(n) => {
                if !self.consts.contains_key(n) {
                    return Err(Diag::new(e.span, format!("неизвестная константа '{n}'")));
                }
                match self.const_value(n, e.span) {
                    Some(v) => v,
                    None => {
                        return Err(Diag::new(
                            e.span,
                            format!("значение константы '{n}' не вычислено"),
                        ))
                    }
                }
            }
            ExprKind::Unary(op, inner) => match (op, self.eval_const(inner)?) {
                (UnOp::Neg, C::Int(n)) => C::Int(n.wrapping_neg()),
                (UnOp::Neg, C::Float(f)) => C::Float(-f),
                (UnOp::Not, C::Bool(b)) => C::Bool(!b),
                (UnOp::BitNot, C::Int(n)) => C::Int(!n),
                _ => {
                    return Err(Diag::new(
                        e.span,
                        "недопустимая унарная операция в константе",
                    ))
                }
            },
            ExprKind::Cast(inner, t) => {
                let v = self.eval_const(inner)?;
                let t = self.resolve_type(t);
                match (v, t) {
                    (C::Int(n), Type::Int) => C::Int(n),
                    (C::Int(n), Type::Float) => C::Float(n as f64),
                    (C::Float(f), Type::Int) => C::Int(f as i64),
                    (C::Float(f), Type::Float) => C::Float(f),
                    (C::Bool(b), Type::Int) => C::Int(b as i64),
                    _ => {
                        return Err(Diag::new(
                            e.span,
                            "недопустимое приведение типа в константе",
                        ))
                    }
                }
            }
            ExprKind::Binary(op, l, r) => {
                let l = self.eval_const(l)?;
                let r = self.eval_const(r)?;
                const_binary(*op, l, r).map_err(|m| Diag::new(e.span, m))?
            }
            _ => return Err(bad(e.span)),
        })
    }

    // ── Functions ────────────────────────────────────────────────────────────

    fn check_fn(&mut self, f: &mut FnDecl) {
        let sig = match self.fns.get(&f.symbol) {
            Some(s) => s.clone(),
            None => return,
        };
        let self_type = if f.has_self {
            Some(sig.params[0].clone())
        } else {
            None
        };
        let mut ctx = FnCtx {
            locals: Vec::new(),
            scopes: vec![HashMap::new()],
            ret: sig.ret.clone(),
            loops: Vec::new(),
            in_defer: 0,
            self_type: self_type.clone(),
        };
        if let Some(st) = &self_type {
            ctx.locals.push(LocalInfo {
                name: "self".into(),
                ty: st.clone(),
                mutable: false,
            });
        }
        let offset = if f.has_self { 1 } else { 0 };
        for (i, p) in f.params.iter().enumerate() {
            if ctx.scopes[0].contains_key(&p.name) {
                self.err(p.span, format!("параметр '{}' повторяется", p.name));
            }
            let id = ctx.locals.len() as LocalId;
            ctx.locals.push(LocalInfo {
                name: p.name.clone(),
                ty: sig.params[i + offset].clone(),
                mutable: false,
            });
            ctx.scopes[0].insert(p.name.clone(), id);
        }
        self.fn_stack.push(ctx);
        let ret = sig.ret.clone();
        let expect = if ret == Type::Void {
            Expect::Discard
        } else {
            Expect::Type(&ret)
        };
        let body_ty = self.block(&mut f.body, expect);
        if ret != Type::Void && !body_ty.is_never() && !body_ty.is_error() && body_ty != ret {
            let span = f.body.tail.as_ref().map(|t| t.span).unwrap_or(f.span);
            if f.body.tail.is_none() {
                self.err_note(
                    span,
                    format!(
                        "функция '{}' должна вернуть значение типа {}",
                        f.symbol, ret
                    ),
                    "добавьте 'return значение;' или выражение без ';' в конце тела",
                );
            } else {
                self.mismatch(span, &ret, &body_ty);
            }
        }
        let ctx = self.fn_stack.pop().unwrap();
        f.locals = ctx.locals;
        f.ret_ty = ret;
    }

    fn ctx(&mut self) -> &mut FnCtx {
        self.fn_stack.last_mut().expect("no function context")
    }

    fn declare_local(&mut self, name: &str, ty: Type, mutable: bool) -> LocalId {
        let ctx = self.ctx();
        let id = ctx.locals.len() as LocalId;
        ctx.locals.push(LocalInfo {
            name: name.to_string(),
            ty,
            mutable,
        });
        ctx.scopes.last_mut().unwrap().insert(name.to_string(), id);
        id
    }

    fn lookup_local(&self, name: &str) -> Option<LocalId> {
        let ctx = self.fn_stack.last()?;
        for scope in ctx.scopes.iter().rev() {
            if let Some(id) = scope.get(name) {
                return Some(*id);
            }
        }
        None
    }

    fn is_outer_local(&self, name: &str) -> bool {
        let n = self.fn_stack.len();
        self.fn_stack[..n.saturating_sub(1)]
            .iter()
            .any(|c| c.scopes.iter().any(|s| s.contains_key(name)))
    }

    // ── Type helpers ─────────────────────────────────────────────────────────

    fn mismatch(&mut self, span: Span, want: &Type, got: &Type) {
        let msg = format!("несовпадение типов: ожидался {want}, найден {got}");
        match (want, got) {
            (Type::Float, Type::Int) => self.err_note(
                span,
                msg,
                "неявных преобразований нет: используйте 'x as float' или литерал вида 1.0",
            ),
            (Type::Int, Type::Float) => self.err_note(
                span,
                msg,
                "используйте 'x as int' (дробная часть отбрасывается)",
            ),
            (Type::Str, Type::Int | Type::Float | Type::Bool) => self.err_note(
                span,
                msg,
                "чтобы получить строку, используйте интерполяцию: \"{x}\"",
            ),
            _ => self.err(span, msg),
        }
    }

    /// Checks that `got` fits `want`. Returns false (after reporting) if not.
    fn expect_type(&mut self, span: Span, want: &Type, got: &Type) -> bool {
        if compatible(want, got) {
            true
        } else {
            self.mismatch(span, want, got);
            false
        }
    }

    // ── Blocks and statements ────────────────────────────────────────────────

    fn block(&mut self, b: &mut Block, expect: Expect) -> Type {
        self.ctx().scopes.push(HashMap::new());
        let mut diverges = false;
        for s in b.stmts.iter_mut() {
            if self.stmt(s) {
                diverges = true;
            }
        }
        let ty = match &mut b.tail {
            Some(t) => {
                let ty = self.expr(t, expect);
                if diverges {
                    Type::Never
                } else {
                    ty
                }
            }
            None if diverges => Type::Never,
            None => Type::Void,
        };
        self.ctx().scopes.pop();
        ty
    }

    /// Returns true if the statement never completes normally.
    fn stmt(&mut self, s: &mut Stmt) -> bool {
        match &mut s.kind {
            StmtKind::Let {
                name,
                mutable,
                ty,
                init,
                local,
            } => {
                let ann = ty.as_ref().map(|t| self.resolve_type(t));
                let got = match &ann {
                    Some(t) => self.expr(init, Expect::Type(t)),
                    None => self.expr(init, Expect::Any),
                };
                let final_ty = match ann {
                    Some(t) => {
                        self.expect_type(init.span, &t, &got);
                        t
                    }
                    None => {
                        if got.has_unknown() {
                            self.err_note(
                                init.span,
                                "не удаётся вывести тип",
                                format!("укажите тип явно: let {name}: [int] = ...;"),
                            );
                            Type::Error
                        } else if !got.is_value() && !got.is_error() {
                            if got.is_never() {
                                Type::Error
                            } else {
                                self.err(
                                    init.span,
                                    format!("выражение типа {got} не имеет значения"),
                                );
                                Type::Error
                            }
                        } else {
                            got.clone()
                        }
                    }
                };
                *local = self.declare_local(name, final_ty, *mutable);
                got.is_never()
            }
            StmtKind::Assign { target, op, value } => {
                let tt = self.place(target);
                let vt = match op {
                    None => self.expr(value, Expect::Type(&tt)),
                    Some(_) => self.expr(value, Expect::Any),
                };
                match op {
                    None => {
                        self.expect_type(value.span, &tt, &vt);
                    }
                    Some(op) => {
                        let res = self.binary_type(s.span, *op, &tt, &vt);
                        if !res.is_error() && !tt.is_error() && res != tt {
                            self.err(
                                s.span,
                                format!(
                                    "операция '{}=' неприменима к {} и {}",
                                    op.symbol(),
                                    tt,
                                    vt
                                ),
                            );
                        }
                    }
                }
                vt.is_never()
            }
            StmtKind::Expr { expr, .. } => self.expr(expr, Expect::Discard).is_never(),
            StmtKind::Defer(e) => {
                self.ctx().in_defer += 1;
                let saved_loops = std::mem::take(&mut self.ctx().loops);
                self.expr(e, Expect::Discard);
                self.ctx().loops = saved_loops;
                self.ctx().in_defer -= 1;
                false
            }
        }
    }

    /// Checks an assignment target and returns its type.
    fn place(&mut self, e: &mut Expr) -> Type {
        match &e.kind {
            ExprKind::Ident(name) => {
                let name = name.clone();
                if let Some(id) = self.lookup_local(&name) {
                    let info = self.ctx().locals[id as usize].clone();
                    if !info.mutable {
                        self.err_note(
                            e.span,
                            format!("нельзя изменить неизменяемую переменную '{name}'"),
                            format!("объявите её через 'var {name} = ...'"),
                        );
                    }
                    e.kind = ExprKind::Local(id);
                    e.ty = info.ty.clone();
                    info.ty
                } else if self.consts.contains_key(&name) {
                    self.err(
                        e.span,
                        format!("нельзя присвоить значение константе '{name}'"),
                    );
                    Type::Error
                } else {
                    self.err(e.span, format!("неизвестная переменная '{name}'"));
                    Type::Error
                }
            }
            ExprKind::Field { .. } | ExprKind::Index { .. } => {
                let t = self.expr(e, Expect::Any);
                if let ExprKind::Index { obj, .. } = &e.kind {
                    if obj.ty == Type::Str {
                        return Type::Error;
                    }
                }
                t
            }
            ExprKind::Paren(_) => {
                self.err(e.span, "лишние скобки в левой части присваивания");
                Type::Error
            }
            ExprKind::SelfRef => {
                self.err(e.span, "нельзя присвоить значение 'self'");
                Type::Error
            }
            _ => {
                self.err(
                    e.span,
                    "в левой части присваивания должна быть переменная, поле или элемент массива",
                );
                Type::Error
            }
        }
    }

    // ── Expressions ──────────────────────────────────────────────────────────

    fn expr(&mut self, e: &mut Expr, expect: Expect) -> Type {
        let ty = self.expr_inner(e, expect);
        e.ty = ty.clone();
        ty
    }

    fn expr_inner(&mut self, e: &mut Expr, expect: Expect) -> Type {
        let span = e.span;
        match &mut e.kind {
            ExprKind::Int(_) => Type::Int,
            ExprKind::Float(_) => Type::Float,
            ExprKind::Bool(_) => Type::Bool,
            ExprKind::Str(_) => Type::Str,
            ExprKind::Interp(parts) => {
                for p in parts.iter_mut() {
                    if let InterpPart::Expr(x) = p {
                        let t = self.expr(x, Expect::Any);
                        if !t.is_printable() && !t.is_error() && !t.is_never() {
                            self.err(
                                x.span,
                                format!("значение типа {t} нельзя вставить в строку"),
                            );
                        }
                    }
                }
                Type::Str
            }
            ExprKind::Paren(inner) => {
                let mut inner = std::mem::replace(inner.as_mut(), Expr::new(ExprKind::Break, span));
                let t = self.expr(&mut inner, expect);
                *e = inner;
                t
            }
            ExprKind::Ident(name) => {
                let name = name.clone();
                self.ident(e, &name)
            }
            ExprKind::SelfRef => match self.fn_stack.last().and_then(|c| c.self_type.clone()) {
                Some(t) => {
                    e.kind = ExprKind::Local(0);
                    t
                }
                None => {
                    self.err(span, "'self' доступен только в методах с параметром self");
                    Type::Error
                }
            },
            ExprKind::Path(ty_name, member) => {
                let (ty_name, member) = (ty_name.clone(), member.clone());
                if let Some(vars) = self.enums.get(&ty_name) {
                    match vars.iter().position(|v| v == &member) {
                        Some(i) => {
                            e.kind = ExprKind::EnumVariant { index: i as u32 };
                            Type::Enum(ty_name)
                        }
                        None => {
                            let symbol = format!("{ty_name}::{member}");
                            if let Some(sig) = self.fns.get(&symbol).cloned() {
                                return self.fn_ref(e, symbol, &sig);
                            }
                            self.err(span, format!("в enum {ty_name} нет варианта '{member}'"));
                            Type::Error
                        }
                    }
                } else if self.struct_idx.contains_key(&ty_name) {
                    let symbol = format!("{ty_name}::{member}");
                    match self.fns.get(&symbol).cloned() {
                        Some(sig) => self.fn_ref(e, symbol, &sig),
                        None => {
                            self.err(span, format!("у типа {ty_name} нет функции '{member}'"));
                            Type::Error
                        }
                    }
                } else {
                    self.err(span, format!("неизвестный тип '{ty_name}'"));
                    Type::Error
                }
            }
            ExprKind::Unary(op, inner) => {
                let op = *op;
                let t = self.expr(inner, Expect::Any);
                match (op, &t) {
                    (_, Type::Error) => Type::Error,
                    (UnOp::Neg, Type::Int | Type::Float) => t,
                    (UnOp::Not, Type::Bool) => Type::Bool,
                    (UnOp::BitNot, Type::Int) => Type::Int,
                    (UnOp::Neg, _) => {
                        self.err(span, format!("унарный '-' неприменим к типу {t}"));
                        Type::Error
                    }
                    (UnOp::Not, _) => {
                        self.err_note(
                            span,
                            format!("'!' применим только к bool, найден {t}"),
                            "для чисел используйте явное сравнение, например 'x == 0'",
                        );
                        Type::Error
                    }
                    (UnOp::BitNot, _) => {
                        self.err(span, format!("'~' применим только к int, найден {t}"));
                        Type::Error
                    }
                }
            }
            ExprKind::Binary(op, l, r) => {
                let op = *op;
                let lt = self.expr(l, Expect::Any);
                let hint = if lt.is_value() {
                    Expect::Type(&lt)
                } else {
                    Expect::Any
                };
                let rt = self.expr(r, hint);
                self.binary_type(span, op, &lt, &rt)
            }
            ExprKind::Pipe(lhs, rhs) => {
                let lhs = std::mem::replace(lhs.as_mut(), Expr::new(ExprKind::Break, span));
                let rhs = std::mem::replace(rhs.as_mut(), Expr::new(ExprKind::Break, span));
                let call = match rhs.kind {
                    ExprKind::Call { callee, mut args } => {
                        args.insert(0, lhs);
                        ExprKind::Call { callee, args }
                    }
                    ExprKind::MethodCall {
                        recv,
                        method,
                        mut args,
                    } => {
                        args.insert(0, lhs);
                        ExprKind::MethodCall { recv, method, args }
                    }
                    _ => ExprKind::Call {
                        callee: Box::new(rhs),
                        args: vec![lhs],
                    },
                };
                e.kind = call;
                self.expr_inner(e, expect)
            }
            ExprKind::Cast(inner, te) => {
                let target = self.resolve_type(te);
                let src = self.expr(inner, Expect::Any);
                let ok = matches!(
                    (&src, &target),
                    (Type::Int, Type::Int | Type::Float)
                        | (Type::Float, Type::Int | Type::Float)
                        | (Type::Bool, Type::Int)
                        | (Type::Enum(_), Type::Int)
                        | (Type::Error, _)
                        | (_, Type::Error)
                        | (Type::Never, _)
                );
                if !ok {
                    let note = match (&src, &target) {
                        (_, Type::Str) => {
                            Some("чтобы получить строку, используйте интерполяцию: \"{x}\"")
                        }
                        (Type::Str, Type::Int) => Some("используйте parse_int(s)"),
                        (Type::Str, Type::Float) => Some("используйте parse_float(s)"),
                        (Type::Int, Type::Bool) => Some("используйте сравнение, например 'x != 0'"),
                        _ => None,
                    };
                    let msg = format!("нельзя привести {src} к {target}");
                    match note {
                        Some(n) => self.err_note(span, msg, n),
                        None => self.err(span, msg),
                    }
                }
                target
            }
            ExprKind::Call { .. } => self.call(e),
            ExprKind::MethodCall { .. } => self.method_call(e),
            ExprKind::Field { obj, name } => {
                let name = name.clone();
                let ot = self.expr(obj, Expect::Any);
                match &ot {
                    Type::Struct(sn) => {
                        let info = &self.structs[self.struct_idx[sn]];
                        match info.fields.iter().position(|(f, _)| f == &name) {
                            Some(i) => {
                                let fty = info.fields[i].1.clone();
                                let obj = std::mem::replace(
                                    obj.as_mut(),
                                    Expr::new(ExprKind::Break, span),
                                );
                                e.kind = ExprKind::FieldIdx {
                                    obj: Box::new(obj),
                                    struct_name: sn.clone(),
                                    index: i as u32,
                                };
                                fty
                            }
                            None => {
                                let has_method = self.fns.contains_key(&format!("{sn}::{name}"));
                                if has_method {
                                    self.err_note(
                                        span,
                                        format!("у {sn} нет поля '{name}'"),
                                        format!("это метод — вызовите его: .{name}()"),
                                    );
                                } else {
                                    self.err(span, format!("у {sn} нет поля '{name}'"));
                                }
                                Type::Error
                            }
                        }
                    }
                    Type::Error => Type::Error,
                    Type::Array(_) | Type::Str if name == "len" => {
                        self.err_note(span, "'len' — это метод", "пишите .len()");
                        Type::Error
                    }
                    _ => {
                        self.err(span, format!("у значения типа {ot} нет полей"));
                        Type::Error
                    }
                }
            }
            ExprKind::Index { obj, idx } => {
                let ot = self.expr(obj, Expect::Any);
                let it = self.expr(idx, Expect::Type(&Type::Int));
                self.expect_type(idx.span, &Type::Int, &it);
                match ot {
                    Type::Array(el) => *el,
                    Type::Error => Type::Error,
                    Type::Str => {
                        self.err_note(
                            span,
                            "строки нельзя индексировать",
                            "используйте s.byte(i) или s.slice(a, b)",
                        );
                        Type::Error
                    }
                    other => {
                        self.err(span, format!("значение типа {other} нельзя индексировать"));
                        Type::Error
                    }
                }
            }
            ExprKind::StructLit { name, fields } => {
                let name = name.clone();
                let Some(&si) = self.struct_idx.get(&name) else {
                    self.err(span, format!("неизвестная структура '{name}'"));
                    for (_, v) in fields.iter_mut() {
                        self.expr(v, Expect::Any);
                    }
                    return Type::Error;
                };
                let decl = self.structs[si].fields.clone();
                let mut slots: Vec<Option<Expr>> = vec![None; decl.len()];
                for (fname, mut value) in std::mem::take(fields) {
                    match decl.iter().position(|(n, _)| n == &fname) {
                        Some(i) => {
                            let ft = decl[i].1.clone();
                            let vt = self.expr(&mut value, Expect::Type(&ft));
                            self.expect_type(value.span, &ft, &vt);
                            if slots[i].is_some() {
                                self.err(value.span, format!("поле '{fname}' указано дважды"));
                            }
                            slots[i] = Some(value);
                        }
                        None => {
                            self.err(value.span, format!("у {name} нет поля '{fname}'"));
                            self.expr(&mut value, Expect::Any);
                        }
                    }
                }
                let missing: Vec<&str> = decl
                    .iter()
                    .zip(&slots)
                    .filter(|(_, s)| s.is_none())
                    .map(|((n, _), _)| n.as_str())
                    .collect();
                if !missing.is_empty() {
                    self.err(
                        span,
                        format!("не заданы поля {}: {}", name, missing.join(", ")),
                    );
                    return Type::Struct(name);
                }
                e.kind = ExprKind::New {
                    struct_name: name.clone(),
                    fields: slots.into_iter().map(|s| s.unwrap()).collect(),
                };
                Type::Struct(name)
            }
            ExprKind::ArrayLit(items) => {
                let mut el: Type = match expect.ty() {
                    Some(Type::Array(t)) => (**t).clone(),
                    _ => Type::Unknown,
                };
                for it in items.iter_mut() {
                    let hint = if el.has_unknown() {
                        Expect::Any
                    } else {
                        Expect::Type(&el)
                    };
                    let t = self.expr(it, hint);
                    if el.has_unknown() {
                        if t.is_value() || t.is_error() {
                            el = t;
                        }
                    } else {
                        let el2 = el.clone();
                        self.expect_type(it.span, &el2, &t);
                    }
                }
                if !items.is_empty() && el == Type::Unknown {
                    self.err(span, "элементы массива не имеют значения");
                    return Type::Error;
                }
                Type::Array(Box::new(el))
            }
            ExprKind::ArrayRepeat { value, count } => {
                let hint = match expect.ty() {
                    Some(Type::Array(t)) => Expect::Type(t.as_ref()),
                    _ => Expect::Any,
                };
                let vt = self.expr(value, hint);
                let ct = self.expr(count, Expect::Type(&Type::Int));
                self.expect_type(count.span, &Type::Int, &ct);
                if !vt.is_value() && !vt.is_error() {
                    self.err(value.span, format!("выражение типа {vt} не имеет значения"));
                    return Type::Error;
                }
                Type::Array(Box::new(vt))
            }
            ExprKind::Lambda { .. } => self.lambda(e),
            ExprKind::If { cond, then, els } => {
                let ct = self.expr(cond, Expect::Type(&Type::Bool));
                self.cond_type(cond.span, &ct);
                let inner_expect = if els.is_none() {
                    Expect::Discard
                } else {
                    expect
                };
                let tt = self.block(then, inner_expect);
                match els {
                    None => {
                        if !matches!(expect, Expect::Discard | Expect::Any)
                            && !matches!(tt, Type::Void | Type::Never | Type::Error)
                        {
                            self.err_note(
                                span,
                                "if без else не возвращает значение",
                                "добавьте ветку else",
                            );
                            return Type::Error;
                        }
                        Type::Void
                    }
                    Some(els) => {
                        let et = self.expr(els, inner_expect);
                        self.unify_branches(span, &tt, &et, expect, "ветки if")
                    }
                }
            }
            ExprKind::Match { scrut, arms } => {
                let st = self.expr(scrut, Expect::Any);
                let scrut_span = scrut.span;
                if !matches!(
                    st,
                    Type::Int | Type::Str | Type::Bool | Type::Enum(_) | Type::Error
                ) {
                    self.err(scrut_span, format!("match по значению типа {st} не поддерживается (допустимы int, str, bool, enum)"));
                }
                let mut result: Option<Type> = None;
                let mut exhaustive = false;
                let mut seen_variants: Vec<u32> = Vec::new();
                let mut seen_bools: Vec<bool> = Vec::new();
                for arm in arms.iter_mut() {
                    for p in arm.pats.iter_mut() {
                        self.pattern(p, &st);
                        match p {
                            Pat::Wildcard(_) => exhaustive = true,
                            Pat::Variant(_, _, _, i) => seen_variants.push(*i),
                            Pat::Bool(b, _) => seen_bools.push(*b),
                            _ => {}
                        }
                    }
                    let at = self.expr(&mut arm.body, expect);
                    result = Some(match result {
                        None => at,
                        Some(prev) => {
                            self.unify_branches(arm.body.span, &prev, &at, expect, "ветки match")
                        }
                    });
                }
                if let Type::Enum(en) = &st {
                    let n = self.enums[en].len() as u32;
                    if (0..n).all(|i| seen_variants.contains(&i)) {
                        exhaustive = true;
                    }
                }
                if st == Type::Bool && seen_bools.contains(&true) && seen_bools.contains(&false) {
                    exhaustive = true;
                }
                let ty = result.unwrap_or(Type::Void);
                if !exhaustive {
                    if matches!(ty, Type::Void | Type::Error | Type::Never)
                        || matches!(expect, Expect::Discard)
                    {
                        Type::Void
                    } else {
                        self.err_note(
                            span,
                            "match возвращает значение, но покрывает не все варианты",
                            "добавьте ветку '_ => ...'",
                        );
                        Type::Error
                    }
                } else {
                    if arms.is_empty() {
                        Type::Never
                    } else {
                        ty
                    }
                }
            }
            ExprKind::Block(b) => self.block(b, expect),
            ExprKind::While { cond, body } => {
                let ct = self.expr(cond, Expect::Type(&Type::Bool));
                self.cond_type(cond.span, &ct);
                self.ctx().loops.push(LoopCtx { has_break: false });
                self.block(body, Expect::Discard);
                self.ctx().loops.pop();
                Type::Void
            }
            ExprKind::For {
                var,
                local,
                iter,
                body,
            } => {
                let var_ty = match iter {
                    ForIter::Range { start, end, .. } => {
                        let a = self.expr(start, Expect::Type(&Type::Int));
                        self.expect_type(start.span, &Type::Int, &a);
                        let b = self.expr(end, Expect::Type(&Type::Int));
                        self.expect_type(end.span, &Type::Int, &b);
                        Type::Int
                    }
                    ForIter::Each(arr) => match self.expr(arr, Expect::Any) {
                        Type::Array(el) => *el,
                        Type::Error => Type::Error,
                        Type::Str => {
                            self.err_note(
                                arr.span,
                                "строку нельзя перебрать в for",
                                "используйте 'for i in 0..s.len()' и s.byte(i)",
                            );
                            Type::Error
                        }
                        other => {
                            self.err(arr.span, format!("for может перебирать только массив или диапазон, найден {other}"));
                            Type::Error
                        }
                    },
                };
                self.ctx().scopes.push(HashMap::new());
                *local = self.declare_local(var, var_ty, false);
                self.ctx().loops.push(LoopCtx { has_break: false });
                self.block(body, Expect::Discard);
                self.ctx().loops.pop();
                self.ctx().scopes.pop();
                Type::Void
            }
            ExprKind::Loop(body) => {
                self.ctx().loops.push(LoopCtx { has_break: false });
                self.block(body, Expect::Discard);
                let lc = self.ctx().loops.pop().unwrap();
                if lc.has_break {
                    Type::Void
                } else {
                    Type::Never
                }
            }
            ExprKind::Break | ExprKind::Continue => {
                let word = if matches!(e.kind, ExprKind::Break) {
                    "break"
                } else {
                    "continue"
                };
                let is_break = word == "break";
                if self.ctx().in_defer > 0 && self.ctx().loops.is_empty() {
                    self.err(span, format!("'{word}' нельзя использовать внутри defer"));
                } else {
                    match self.ctx().loops.last_mut() {
                        Some(l) => {
                            if is_break {
                                l.has_break = true;
                            }
                        }
                        None => self.err(span, format!("'{word}' вне цикла")),
                    }
                }
                Type::Never
            }
            ExprKind::Return(value) => {
                if self.ctx().in_defer > 0 {
                    self.err(span, "'return' нельзя использовать внутри defer");
                }
                let ret = self.ctx().ret.clone();
                match value {
                    Some(v) => {
                        let vt = self.expr(v, Expect::Type(&ret));
                        if ret == Type::Void {
                            if vt != Type::Void && !vt.is_error() {
                                self.err(
                                    v.span,
                                    "функция ничего не возвращает, а return передаёт значение",
                                );
                            }
                        } else {
                            self.expect_type(v.span, &ret, &vt);
                        }
                    }
                    None => {
                        if ret != Type::Void {
                            self.err(
                                span,
                                format!("return без значения в функции, возвращающей {ret}"),
                            );
                        }
                    }
                }
                Type::Never
            }
            // Resolved forms never come from the parser.
            _ => e.ty.clone(),
        }
    }

    fn cond_type(&mut self, span: Span, t: &Type) {
        if !matches!(t, Type::Bool | Type::Error | Type::Never) {
            self.err_note(
                span,
                format!("условие должно иметь тип bool, найден {t}"),
                "используйте явное сравнение, например 'x != 0'",
            );
        }
    }

    fn unify_branches(
        &mut self,
        span: Span,
        a: &Type,
        b: &Type,
        expect: Expect,
        what: &str,
    ) -> Type {
        if a.is_error() || b.is_error() {
            return Type::Error;
        }
        if a.is_never() {
            return b.clone();
        }
        if b.is_never() {
            return a.clone();
        }
        if a == b {
            return a.clone();
        }
        if a.has_unknown() && compatible(b, a) {
            return b.clone();
        }
        if b.has_unknown() && compatible(a, b) {
            return a.clone();
        }
        if matches!(expect, Expect::Discard) {
            return Type::Void;
        }
        self.err(span, format!("{what} имеют разные типы: {a} и {b}"));
        Type::Error
    }

    fn pattern(&mut self, p: &mut Pat, st: &Type) {
        let ok = match (&mut *p, st) {
            (_, Type::Error) | (Pat::Wildcard(_), _) => true,
            (Pat::Int(..) | Pat::Range(..), Type::Int) => true,
            (Pat::Str(..), Type::Str) => true,
            (Pat::Bool(..), Type::Bool) => true,
            (Pat::Variant(en, v, span, idx), Type::Enum(se)) => {
                if en != se {
                    let span = *span;
                    self.err(
                        span,
                        format!(
                            "образец относится к enum {en}, а сопоставляется значение типа {se}"
                        ),
                    );
                } else {
                    match self.enums[se].iter().position(|x| x == v) {
                        Some(i) => *idx = i as u32,
                        None => {
                            let span = *span;
                            self.err(span, format!("в enum {se} нет варианта '{v}'"));
                        }
                    }
                }
                true
            }
            _ => false,
        };
        if !ok {
            self.err(
                p.span(),
                format!("образец не подходит к значению типа {st}"),
            );
        }
        if let Pat::Range(lo, hi, inc, span) = p {
            if (*inc && lo > hi) || (!*inc && lo >= hi) {
                let span = *span;
                self.err(span, "пустой диапазон в образце");
            }
        }
    }

    fn binary_type(&mut self, span: Span, op: BinOp, l: &Type, r: &Type) -> Type {
        use BinOp::*;
        if l.is_error() || r.is_error() {
            return Type::Error;
        }
        let res = match op {
            Add | Sub | Mul | Div | Rem | Pow => match (l, r) {
                (Type::Int, Type::Int) => Some(Type::Int),
                (Type::Float, Type::Float) => Some(Type::Float),
                (Type::Str, Type::Str) if op == Add => Some(Type::Str),
                _ => None,
            },
            BitAnd | BitOr | BitXor | Shl | Shr => match (l, r) {
                (Type::Int, Type::Int) => Some(Type::Int),
                _ => None,
            },
            Eq | Ne => {
                if l == r
                    && matches!(
                        l,
                        Type::Int | Type::Float | Type::Bool | Type::Str | Type::Enum(_)
                    )
                {
                    Some(Type::Bool)
                } else if l == r {
                    self.err(
                        span,
                        format!(
                            "значения типа {l} нельзя сравнивать через '{}'",
                            op.symbol()
                        ),
                    );
                    return Type::Error;
                } else {
                    None
                }
            }
            Lt | Le | Gt | Ge => {
                if l == r && matches!(l, Type::Int | Type::Float | Type::Str) {
                    Some(Type::Bool)
                } else {
                    None
                }
            }
            And | Or => match (l, r) {
                (Type::Bool, Type::Bool) => Some(Type::Bool),
                _ => None,
            },
        };
        match res {
            Some(t) => t,
            None if l.is_never() || r.is_never() => Type::Never,
            None => {
                let msg = format!("операция '{}' неприменима к {} и {}", op.symbol(), l, r);
                match (l, r) {
                    (Type::Int, Type::Float) | (Type::Float, Type::Int) => self.err_note(
                        span,
                        msg,
                        "неявных преобразований нет: используйте 'as float' или 'as int'",
                    ),
                    (Type::Str, _) | (_, Type::Str) if op == Add => self.err_note(
                        span,
                        msg,
                        "для сборки строки используйте интерполяцию: \"{a}{b}\"",
                    ),
                    _ => self.err(span, msg),
                }
                Type::Error
            }
        }
    }

    // ── Names ────────────────────────────────────────────────────────────────

    fn ident(&mut self, e: &mut Expr, name: &str) -> Type {
        if let Some(id) = self.lookup_local(name) {
            e.kind = ExprKind::Local(id);
            return self.ctx().locals[id as usize].ty.clone();
        }
        if self.consts.contains_key(name) {
            return match self.const_value(name, e.span) {
                Some(v) => {
                    *e = v.to_expr(e.span);
                    v.ty()
                }
                None => Type::Error,
            };
        }
        if let Some(sig) = self.fns.get(name).cloned() {
            return self.fn_ref(e, name.to_string(), &sig);
        }
        if self.is_outer_local(name) {
            self.err_note(
                e.span,
                format!("лямбда не может захватывать локальную переменную '{name}'"),
                "передайте значение через параметр",
            );
            return Type::Error;
        }
        if self.externs.contains_key(name) {
            self.err(
                e.span,
                format!("внешнюю функцию '{name}' нельзя использовать как значение"),
            );
            return Type::Error;
        }
        if Builtin::function(name).is_some() {
            self.err(
                e.span,
                format!("встроенную функцию '{name}' нельзя использовать как значение"),
            );
            return Type::Error;
        }
        if self.type_exists(name) {
            self.err(e.span, format!("'{name}' — это тип, а не значение"));
            return Type::Error;
        }
        let hint = match name {
            "println!" | "print!" => Some("println — обычная функция, без '!'"),
            "null" | "nil" | "None" => Some("в Orbitron нет null"),
            "this" => Some("используйте 'self'"),
            _ => None,
        };
        match hint {
            Some(h) => self.err_note(e.span, format!("неизвестное имя '{name}'"), h),
            None => self.err(e.span, format!("неизвестное имя '{name}'")),
        }
        Type::Error
    }

    fn fn_ref(&mut self, e: &mut Expr, symbol: String, sig: &FnSig) -> Type {
        e.kind = ExprKind::FnRef(symbol);
        Type::Fn(sig.params.clone(), Box::new(sig.ret.clone()))
    }

    // ── Calls ────────────────────────────────────────────────────────────────

    fn check_args(&mut self, span: Span, what: &str, params: &[Type], args: &mut [Expr]) {
        if params.len() != args.len() {
            self.err(
                span,
                format!(
                    "{what} ожидает {} {}, передано {}",
                    params.len(),
                    plural_args(params.len()),
                    args.len()
                ),
            );
            for a in args.iter_mut() {
                self.expr(a, Expect::Any);
            }
            return;
        }
        for (a, p) in args.iter_mut().zip(params) {
            let t = self.expr(a, Expect::Type(p));
            self.expect_type(a.span, p, &t);
        }
    }

    fn call(&mut self, e: &mut Expr) -> Type {
        let span = e.span;
        let ExprKind::Call { callee, args } = &mut e.kind else {
            unreachable!()
        };
        let mut args = std::mem::take(args);
        let mut callee = std::mem::replace(callee.as_mut(), Expr::new(ExprKind::Break, span));

        // Direct calls by name.
        if let ExprKind::Ident(name) = &callee.kind {
            let name = name.clone();
            if self.lookup_local(&name).is_none() {
                if let Some(sig) = self.fns.get(&name).cloned() {
                    self.check_args(span, &format!("функция '{name}'"), &sig.params, &mut args);
                    e.kind = ExprKind::CallFn { symbol: name, args };
                    return sig.ret;
                }
                if let Some(sig) = self.externs.get(&name).cloned() {
                    self.check_args(span, &format!("функция '{name}'"), &sig.params, &mut args);
                    e.kind = ExprKind::CallExtern { name, args };
                    return sig.ret;
                }
                if let Some(b) = Builtin::function(&name) {
                    if !self.consts.contains_key(&name) {
                        let t = self.builtin(span, b, &name, &mut args);
                        e.kind = ExprKind::CallBuiltin { builtin: b, args };
                        return t;
                    }
                }
            }
        }
        if let ExprKind::Path(ty_name, member) = &callee.kind {
            let symbol = format!("{ty_name}::{member}");
            if let Some(sig) = self.fns.get(&symbol).cloned() {
                if sig.has_self {
                    self.err_note(
                        span,
                        format!("'{symbol}' — метод экземпляра"),
                        format!("вызывайте его через значение: x.{member}(...)"),
                    );
                    return Type::Error;
                }
                self.check_args(span, &format!("функция '{symbol}'"), &sig.params, &mut args);
                e.kind = ExprKind::CallFn { symbol, args };
                return sig.ret;
            }
        }

        // Indirect call through a function value.
        let ct = self.expr(&mut callee, Expect::Any);
        match ct {
            Type::Fn(params, ret) => {
                self.check_args(span, "функция", &params, &mut args);
                e.kind = ExprKind::CallIndirect {
                    callee: Box::new(callee),
                    args,
                };
                *ret
            }
            Type::Error => {
                for a in args.iter_mut() {
                    self.expr(a, Expect::Any);
                }
                Type::Error
            }
            other => {
                self.err(span, format!("значение типа {other} нельзя вызвать"));
                Type::Error
            }
        }
    }

    fn method_call(&mut self, e: &mut Expr) -> Type {
        let span = e.span;
        let ExprKind::MethodCall { recv, method, args } = &mut e.kind else {
            unreachable!()
        };
        let method = method.clone();
        let mut args = std::mem::take(args);
        let mut recv = std::mem::replace(recv.as_mut(), Expr::new(ExprKind::Break, span));

        // `Type::method` style is a Path; here the receiver is a value.
        let rt = self.expr(&mut recv, Expect::Any);
        match &rt {
            Type::Struct(n) | Type::Enum(n) => {
                let symbol = format!("{n}::{method}");
                if let Some(sig) = self.fns.get(&symbol).cloned() {
                    if !sig.has_self {
                        self.err_note(
                            span,
                            format!("'{symbol}' — функция типа, а не метод"),
                            format!("вызывайте её как {n}::{method}(...)"),
                        );
                        return Type::Error;
                    }
                    self.check_args(
                        span,
                        &format!("метод '{symbol}'"),
                        &sig.params[1..],
                        &mut args,
                    );
                    args.insert(0, recv);
                    e.kind = ExprKind::CallFn { symbol, args };
                    return sig.ret;
                }
                // A field holding a function: `obj.callback(x)`.
                if let Type::Struct(sn) = &rt {
                    let info = &self.structs[self.struct_idx[sn]];
                    if let Some(i) = info.fields.iter().position(|(f, _)| f == &method) {
                        if let Type::Fn(params, ret) = info.fields[i].1.clone() {
                            self.check_args(
                                span,
                                &format!("поле-функция '{method}'"),
                                &params,
                                &mut args,
                            );
                            let ft = Type::Fn(params, ret.clone());
                            let field = Expr {
                                kind: ExprKind::FieldIdx {
                                    obj: Box::new(recv),
                                    struct_name: sn.clone(),
                                    index: i as u32,
                                },
                                span,
                                ty: ft,
                            };
                            e.kind = ExprKind::CallIndirect {
                                callee: Box::new(field),
                                args,
                            };
                            return *ret;
                        }
                    }
                }
                self.err(span, format!("у типа {n} нет метода '{method}'"));
                Type::Error
            }
            Type::Array(el) => {
                let el = (**el).clone();
                let (b, params, ret) = match method.as_str() {
                    "len" => (Builtin::ArrLen, vec![], Type::Int),
                    "push" => (Builtin::ArrPush, vec![el.clone()], Type::Void),
                    "pop" => (Builtin::ArrPop, vec![], el.clone()),
                    _ => {
                        self.err_note(
                            span,
                            format!("у массива нет метода '{method}'"),
                            "доступны: len(), push(x), pop()",
                        );
                        return Type::Error;
                    }
                };
                self.check_args(span, &format!("метод '{method}'"), &params, &mut args);
                args.insert(0, recv);
                e.kind = ExprKind::CallBuiltin { builtin: b, args };
                ret
            }
            Type::Str => {
                let (b, params, ret) = match method.as_str() {
                    "len" => (Builtin::StrLen, vec![], Type::Int),
                    "byte" => (Builtin::StrByte, vec![Type::Int], Type::Int),
                    "slice" => (Builtin::StrSlice, vec![Type::Int, Type::Int], Type::Str),
                    "contains" => (Builtin::StrContains, vec![Type::Str], Type::Bool),
                    "find" => (Builtin::StrFind, vec![Type::Str], Type::Int),
                    _ => {
                        self.err_note(
                            span,
                            format!("у строки нет метода '{method}'"),
                            "доступны: len(), byte(i), slice(a, b), contains(s), find(s)",
                        );
                        return Type::Error;
                    }
                };
                self.check_args(span, &format!("метод '{method}'"), &params, &mut args);
                args.insert(0, recv);
                e.kind = ExprKind::CallBuiltin { builtin: b, args };
                ret
            }
            Type::Error => Type::Error,
            other => {
                self.err(span, format!("у значения типа {other} нет методов"));
                Type::Error
            }
        }
    }

    fn builtin(&mut self, span: Span, b: Builtin, name: &str, args: &mut [Expr]) -> Type {
        use Builtin::*;
        let fixed = |params: Vec<Type>, ret: Type| (Some(params), ret);
        let (params, ret) = match b {
            Print | Println | Eprintln => {
                if args.len() > 1 || (args.is_empty() && b == Print) {
                    self.err_note(
                        span,
                        format!("{name} принимает одно значение"),
                        "несколько значений объединяйте интерполяцией: \"{a} {b}\"",
                    );
                }
                for a in args.iter_mut() {
                    let t = self.expr(a, Expect::Any);
                    if !t.is_printable() && !t.is_error() && !t.is_never() {
                        self.err(a.span, format!("значение типа {t} нельзя напечатать"));
                    }
                }
                return Type::Void;
            }
            Assert => {
                if args.is_empty() || args.len() > 2 {
                    self.err(span, "assert ожидает условие и необязательное сообщение: assert(cond, \"текст\")");
                }
                for (i, a) in args.iter_mut().enumerate() {
                    let want = if i == 0 { Type::Bool } else { Type::Str };
                    let t = self.expr(a, Expect::Type(&want));
                    self.expect_type(a.span, &want, &t);
                }
                return Type::Void;
            }
            Abs | Min | Max => {
                let n = if b == Abs { 1 } else { 2 };
                if args.len() != n {
                    self.err(span, format!("{name} ожидает {n} {}", plural_args(n)));
                    return Type::Error;
                }
                let t0 = self.expr(&mut args[0], Expect::Any);
                if !matches!(t0, Type::Int | Type::Float | Type::Error) {
                    self.err(
                        args[0].span,
                        format!("{name} применим к int или float, найден {t0}"),
                    );
                    return Type::Error;
                }
                if n == 2 {
                    let t1 = self.expr(&mut args[1], Expect::Type(&t0));
                    self.expect_type(args[1].span, &t0, &t1);
                }
                return t0;
            }
            ReadLine => fixed(vec![], Type::Str),
            ParseInt => fixed(vec![Type::Str], Type::Int),
            ParseFloat => fixed(vec![Type::Str], Type::Float),
            Chr => fixed(vec![Type::Int], Type::Str),
            Exit => fixed(vec![Type::Int], Type::Never),
            Panic => fixed(vec![Type::Str], Type::Never),
            Clock => fixed(vec![], Type::Float),
            Sqrt | Sin | Cos | Tan | Exp | Ln | Floor | Ceil | Round => {
                fixed(vec![Type::Float], Type::Float)
            }
            Atan2 => fixed(vec![Type::Float, Type::Float], Type::Float),
            _ => unreachable!("method builtin used as function"),
        };
        if let Some(params) = params {
            self.check_args(span, &format!("функция '{name}'"), &params, args);
        }
        ret
    }

    fn lambda(&mut self, e: &mut Expr) -> Type {
        let span = e.span;
        let ExprKind::Lambda { params, ret, body } =
            std::mem::replace(&mut e.kind, ExprKind::Break)
        else {
            unreachable!()
        };
        self.lambda_counter += 1;
        let symbol = format!("lambda#{}", self.lambda_counter);
        let mut f = FnDecl {
            name: symbol.clone(),
            has_self: false,
            params,
            ret,
            body,
            span,
            symbol: symbol.clone(),
            locals: Vec::new(),
            ret_ty: Type::Unknown,
        };
        self.declare_fn(&mut f, None);
        self.check_fn(&mut f);
        let sig = self.fns[&symbol].clone();
        self.lambdas.push(f);
        e.kind = ExprKind::FnRef(symbol);
        Type::Fn(sig.params, Box::new(sig.ret))
    }
}

/// `got` can be used where `want` is expected.
fn compatible(want: &Type, got: &Type) -> bool {
    if want == got || got.is_never() || got.is_error() || want.is_error() {
        return true;
    }
    match (want, got) {
        (Type::Array(a), Type::Array(b)) => {
            **b == Type::Unknown || compatible(a, b) && compatible(b, a)
        }
        _ => false,
    }
}

fn plural_args(n: usize) -> &'static str {
    let (m10, m100) = (n % 10, n % 100);
    if m10 == 1 && m100 != 11 {
        "аргумент"
    } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
        "аргумента"
    } else {
        "аргументов"
    }
}

fn const_binary(op: BinOp, l: ConstVal, r: ConstVal) -> Result<ConstVal, String> {
    use BinOp::*;
    use ConstVal as C;
    Ok(match (l, r) {
        (C::Int(a), C::Int(b)) => match op {
            Add => C::Int(a.wrapping_add(b)),
            Sub => C::Int(a.wrapping_sub(b)),
            Mul => C::Int(a.wrapping_mul(b)),
            Div | Rem if b == 0 => return Err("деление на ноль в константе".into()),
            Div => C::Int(a.wrapping_div(b)),
            Rem => C::Int(a.wrapping_rem(b)),
            Pow => {
                if b < 0 {
                    return Err("отрицательная степень целого числа".into());
                }
                C::Int(a.wrapping_pow(b.min(u32::MAX as i64) as u32))
            }
            BitAnd => C::Int(a & b),
            BitOr => C::Int(a | b),
            BitXor => C::Int(a ^ b),
            Shl => C::Int(a.wrapping_shl((b & 63) as u32)),
            Shr => C::Int(a.wrapping_shr((b & 63) as u32)),
            Eq => C::Bool(a == b),
            Ne => C::Bool(a != b),
            Lt => C::Bool(a < b),
            Le => C::Bool(a <= b),
            Gt => C::Bool(a > b),
            Ge => C::Bool(a >= b),
            And | Or => return Err("'&&' и '||' применимы только к bool".into()),
        },
        (C::Float(a), C::Float(b)) => match op {
            Add => C::Float(a + b),
            Sub => C::Float(a - b),
            Mul => C::Float(a * b),
            Div => C::Float(a / b),
            Rem => C::Float(a % b),
            Pow => C::Float(a.powf(b)),
            Eq => C::Bool(a == b),
            Ne => C::Bool(a != b),
            Lt => C::Bool(a < b),
            Le => C::Bool(a <= b),
            Gt => C::Bool(a > b),
            Ge => C::Bool(a >= b),
            _ => return Err(format!("операция '{}' неприменима к float", op.symbol())),
        },
        (C::Bool(a), C::Bool(b)) => match op {
            And => C::Bool(a && b),
            Or => C::Bool(a || b),
            Eq => C::Bool(a == b),
            Ne => C::Bool(a != b),
            _ => return Err(format!("операция '{}' неприменима к bool", op.symbol())),
        },
        (C::Str(a), C::Str(b)) => match op {
            Add => C::Str(a + &b),
            Eq => C::Bool(a == b),
            Ne => C::Bool(a != b),
            _ => return Err(format!("операция '{}' неприменима к str", op.symbol())),
        },
        (l, r) => {
            return Err(format!(
                "операция '{}' неприменима к {} и {}",
                op.symbol(),
                l.ty(),
                r.ty()
            ))
        }
    })
}
