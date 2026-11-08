//! JVM back end: emits Java source (`Main.java`), which is compiled with
//! `javac` together with the runtime (`runtime/OtRt.java`) and packed into a jar.
//!
//! Orbitron is expression-oriented while Java is not, so block-like
//! expressions (`if`, `match`, blocks) used as values are lowered into
//! statements that assign a temporary. Operands are spilled into temporaries
//! whenever a later operand needs statements, preserving evaluation order.

use std::fmt::Write;

use crate::ast::*;
use crate::check::Module;
use crate::diag::Span;
use crate::types::Type;

const JAVA_KEYWORDS: &[&str] = &[
    "abstract",
    "assert",
    "boolean",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extends",
    "final",
    "finally",
    "float",
    "for",
    "goto",
    "if",
    "implements",
    "import",
    "instanceof",
    "int",
    "interface",
    "long",
    "native",
    "new",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "short",
    "static",
    "strictfp",
    "super",
    "switch",
    "synchronized",
    "this",
    "throw",
    "throws",
    "transient",
    "try",
    "void",
    "volatile",
    "while",
    "true",
    "false",
    "null",
    "var",
    "record",
    "yield",
    "sealed",
    "permits",
    "_",
];

fn ident(name: &str) -> String {
    if JAVA_KEYWORDS.contains(&name) {
        format!("{name}_")
    } else {
        name.to_string()
    }
}

fn fn_name(symbol: &str) -> String {
    format!("fn_{}", symbol.replace("::", "__").replace('#', "_"))
}

fn java_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\{:03o}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn float_lit(f: f64) -> String {
    if f.is_nan() {
        "Double.NaN".into()
    } else if f.is_infinite() {
        if f > 0.0 {
            "Double.POSITIVE_INFINITY".into()
        } else {
            "Double.NEGATIVE_INFINITY".into()
        }
    } else {
        let s = format!("{f:?}");
        if s.contains('.') || s.contains('e') || s.contains('E') {
            s
        } else {
            format!("{s}.0")
        }
    }
}

fn int_lit(n: i64) -> String {
    if n == i64::MIN {
        "(-9223372036854775807L - 1L)".into()
    } else {
        format!("{n}L")
    }
}

#[derive(Clone)]
enum Target {
    Discard,
    Assign(String),
    Return,
}

struct Operand {
    code: String,
    stmts: Vec<String>,
    pure: bool,
}

pub struct JavaGen<'m> {
    m: &'m Module,
    files: &'m [String],
    ifaces: Vec<(Vec<Type>, Type)>,
    // per-function state
    lines: Vec<String>,
    indent: usize,
    tmp: usize,
    dead: bool,
    local_names: Vec<String>,
    local_tys: Vec<Type>,
    loop_depths: Vec<usize>,
    defers: Vec<Vec<Expr>>,
    ret_ty: Type,
}

pub fn emit(m: &Module, files: &[String]) -> String {
    let mut g = JavaGen {
        m,
        files,
        ifaces: Vec::new(),
        lines: Vec::new(),
        indent: 0,
        tmp: 0,
        dead: false,
        local_names: Vec::new(),
        local_tys: Vec::new(),
        loop_depths: Vec::new(),
        defers: Vec::new(),
        ret_ty: Type::Void,
    };
    g.run()
}

impl<'m> JavaGen<'m> {
    fn run(&mut self) -> String {
        let mut methods = String::new();
        for f in &self.m.fns {
            methods.push_str(&self.function(f));
        }

        let mut out = String::new();
        out.push_str("// Сгенерировано компилятором Orbitron\n");
        out.push_str("import java.util.ArrayList;\n\n");
        out.push_str("@SuppressWarnings({\"unused\", \"unchecked\"})\npublic final class Main {\n");
        for en in &self.m.enums {
            let names: Vec<String> = en.variants.iter().map(|v| java_str(v)).collect();
            let _ = writeln!(
                out,
                "    static final String[] E_{} = {{{}}};",
                en.name,
                names.join(", ")
            );
        }
        for s in &self.m.structs {
            let _ = writeln!(out, "\n    static final class S_{} {{", s.name);
            let mut params = Vec::new();
            for (n, t) in &s.fields {
                let jt = self.jty(t);
                let _ = writeln!(out, "        {jt} {};", ident(n));
                params.push(format!("{jt} {}", ident(n)));
            }
            let _ = writeln!(out, "        S_{}({}) {{", s.name, params.join(", "));
            for (n, _) in &s.fields {
                let _ = writeln!(out, "            this.{0} = {0};", ident(n));
            }
            out.push_str("        }\n    }\n");
        }
        // Interfaces can be registered while generating struct fields, so
        // emit them after everything else has been visited.
        let mut ifaces = String::new();
        for (i, (ps, r)) in self.ifaces.clone().iter().enumerate() {
            let params: Vec<String> = ps
                .iter()
                .enumerate()
                .map(|(k, p)| format!("{} a{k}", self.jty(p)))
                .collect();
            let _ = writeln!(
                ifaces,
                "    interface F{i} {{ {} call({}); }}",
                self.jty(r),
                params.join(", ")
            );
        }
        out.push_str(&ifaces);
        out.push('\n');
        out.push_str(&methods);

        if let Some(main) = self.m.fns.iter().find(|f| f.symbol == "main") {
            out.push_str("    public static void main(String[] args) {\n");
            if main.ret_ty == Type::Int {
                let _ = writeln!(
                    out,
                    "        OtRt.run(() -> {{ throw OtRt.exit({}()); }});",
                    fn_name("main")
                );
            } else {
                let _ = writeln!(out, "        OtRt.run(() -> {}());", fn_name("main"));
            }
            out.push_str("    }\n");
        }
        out.push_str("}\n");
        out
    }

    // ── Types ────────────────────────────────────────────────────────────────

    fn iface(&mut self, ps: &[Type], r: &Type) -> String {
        let key = (ps.to_vec(), r.clone());
        let idx = match self.ifaces.iter().position(|k| *k == key) {
            Some(i) => i,
            None => {
                self.ifaces.push(key);
                self.ifaces.len() - 1
            }
        };
        format!("F{idx}")
    }

    fn jty(&mut self, t: &Type) -> String {
        match t {
            Type::Int | Type::Enum(_) => "long".into(),
            Type::Float => "double".into(),
            Type::Bool => "boolean".into(),
            Type::Str => "String".into(),
            Type::Void | Type::Never => "void".into(),
            Type::Array(el) => format!("ArrayList<{}>", self.boxed(el)),
            Type::Struct(n) => format!("S_{n}"),
            Type::Fn(ps, r) => self.iface(ps, r),
            Type::Unknown | Type::Error => panic!("unresolved type in codegen"),
        }
    }

    fn boxed(&mut self, t: &Type) -> String {
        match t {
            Type::Int | Type::Enum(_) => "Long".into(),
            Type::Float => "Double".into(),
            Type::Bool => "Boolean".into(),
            other => self.jty(other),
        }
    }

    fn default_value(t: &Type) -> &'static str {
        match t {
            Type::Int | Type::Enum(_) => "0L",
            Type::Float => "0.0",
            Type::Bool => "false",
            _ => "null",
        }
    }

    /// Element reads from generic containers must be unboxed explicitly so
    /// that `==` compares values, not references.
    fn unbox(&mut self, code: String, t: &Type) -> String {
        match t {
            Type::Int | Type::Enum(_) => format!("((long) {code})"),
            Type::Float => format!("((double) {code})"),
            Type::Bool => format!("((boolean) {code})"),
            _ => code,
        }
    }

    // ── Emission helpers ─────────────────────────────────────────────────────

    fn line(&mut self, s: impl AsRef<str>) {
        if self.dead {
            return;
        }
        let pad = "    ".repeat(self.indent);
        self.lines.push(format!("{pad}{}", s.as_ref()));
    }

    fn tmp(&mut self) -> String {
        self.tmp += 1;
        format!("t${}", self.tmp)
    }

    fn loc(&self, span: Span) -> String {
        let file = self
            .files
            .get(span.file as usize)
            .map(|s| s.as_str())
            .unwrap_or("?");
        java_str(&format!("{}:{}:{}", file, span.line, span.col))
    }

    /// Runs `f` as a separately reachable branch; returns whether it ended dead.
    fn branch(&mut self, f: impl FnOnce(&mut Self)) -> bool {
        self.dead = false;
        self.indent += 1;
        f(self);
        self.indent -= 1;
        let d = self.dead;
        self.dead = false;
        d
    }

    // ── Functions ────────────────────────────────────────────────────────────

    fn function(&mut self, f: &FnDecl) -> String {
        self.lines.clear();
        self.indent = 2;
        self.tmp = 0;
        self.dead = false;
        self.loop_depths.clear();
        self.defers.clear();
        self.ret_ty = f.ret_ty.clone();
        self.local_names = f
            .locals
            .iter()
            .enumerate()
            .map(|(i, l)| {
                if l.name == "self" {
                    "self".to_string()
                } else {
                    format!("{}_{i}", l.name)
                }
            })
            .collect();
        self.local_tys = f.locals.iter().map(|l| l.ty.clone()).collect();

        let n_params = f.params.len() + usize::from(f.has_self);
        let mut params = Vec::new();
        for i in 0..n_params {
            let t = self.local_tys[i].clone();
            params.push(format!("{} {}", self.jty(&t), self.local_names[i]));
        }
        let ret = self.jty(&f.ret_ty.clone());

        let target = if f.ret_ty == Type::Void {
            Target::Discard
        } else {
            Target::Return
        };
        self.block_into(&f.body, target);

        let mut out = String::new();
        let _ = writeln!(
            out,
            "    static {ret} {}({}) {{",
            fn_name(&f.symbol),
            params.join(", ")
        );
        for l in &self.lines {
            out.push_str(l);
            out.push('\n');
        }
        out.push_str("    }\n\n");
        out
    }

    fn emit_defers(&mut self, from_depth: usize) {
        let pending: Vec<Expr> = self.defers[from_depth..]
            .iter()
            .rev()
            .flat_map(|scope| scope.iter().rev().cloned())
            .collect();
        let saved = std::mem::take(&mut self.defers);
        for d in &pending {
            self.expr_stmt(d);
        }
        self.defers = saved;
    }

    fn emit_return(&mut self, value: Option<String>, ty: &Type) {
        let has_defers = self.defers.iter().any(|s| !s.is_empty());
        let value = match value {
            Some(v) if has_defers => {
                let t = self.tmp();
                let jt = self.jty(ty);
                self.line(format!("{jt} {t} = {v};"));
                Some(t)
            }
            v => v,
        };
        self.emit_defers(0);
        match value {
            Some(v) => self.line(format!("return {v};")),
            None => self.line("return;"),
        }
        self.dead = true;
    }

    /// Emits the statements of `b` (without braces) and routes its tail value.
    fn block_into(&mut self, b: &Block, target: Target) {
        self.defers.push(Vec::new());
        for s in &b.stmts {
            self.stmt(s);
        }
        if let Some(t) = &b.tail {
            match &target {
                Target::Discard => self.expr_stmt(t),
                Target::Assign(var) => {
                    if t.is_block_like() {
                        self.expr_into(t, Target::Assign(var.clone()));
                    } else {
                        let v = self.expr(t);
                        if !t.ty.is_never() {
                            self.line(format!("{var} = {v};"));
                        }
                    }
                }
                Target::Return => {
                    if t.is_block_like() {
                        self.expr_into(t, Target::Return);
                    } else {
                        let v = self.expr(t);
                        if !self.dead && !t.ty.is_never() {
                            let ty = self.ret_ty.clone();
                            self.emit_return(Some(v), &ty);
                        }
                    }
                }
            }
        }
        let depth = self.defers.len() - 1;
        if !self.dead && !self.defers[depth].is_empty() {
            self.emit_defers(depth);
        }
        self.defers.pop();
    }

    fn stmt(&mut self, s: &Stmt) {
        if self.dead {
            return;
        }
        match &s.kind {
            StmtKind::Let { init, local, .. } => {
                let name = self.local_names[*local as usize].clone();
                let ty = self.local_tys[*local as usize].clone();
                let jt = self.jty(&ty);
                if init.is_block_like() {
                    self.line(format!("{jt} {name} = {};", Self::default_value(&ty)));
                    self.expr_into(init, Target::Assign(name));
                } else {
                    let v = self.expr(init);
                    self.line(format!("{jt} {name} = {v};"));
                }
            }
            StmtKind::Assign { target, op, value } => self.assign(target, *op, value, s.span),
            StmtKind::Expr { expr, .. } => self.expr_stmt(expr),
            StmtKind::Defer(e) => self.defers.last_mut().unwrap().push(e.clone()),
        }
    }

    fn assign(&mut self, target: &Expr, op: Option<BinOp>, value: &Expr, span: Span) {
        match &target.kind {
            ExprKind::Local(id) => {
                let name = self.local_names[*id as usize].clone();
                let v = self.expr(value);
                let v = match op {
                    Some(op) => self.binop(op, &target.ty, &name, &v, span),
                    None => v,
                };
                self.line(format!("{name} = {v};"));
            }
            ExprKind::FieldIdx {
                obj,
                struct_name,
                index,
            } => {
                let fname = ident(&self.m.struct_info(struct_name).fields[*index as usize].0);
                let o = self.expr(obj);
                let o = if op.is_some() || self.has_effects(value) {
                    self.spill(&o, &obj.ty)
                } else {
                    o
                };
                let v = self.expr(value);
                let v = match op {
                    Some(op) => self.binop(op, &target.ty, &format!("{o}.{fname}"), &v, span),
                    None => v,
                };
                self.line(format!("{o}.{fname} = {v};"));
            }
            ExprKind::Index { obj, idx } => {
                let a = self.expr(obj);
                let a = self.spill(&a, &obj.ty);
                let i = self.expr(idx);
                let i = self.spill(&i, &Type::Int);
                let loc = self.loc(target.span);
                let v = self.expr(value);
                let v = match op {
                    Some(op) => {
                        let old = format!("OtRt.get({a}, {i}, {loc})");
                        let old = self.unbox(old, &target.ty);
                        let old = self.spill(&old, &target.ty);
                        self.binop(op, &target.ty, &old, &v, span)
                    }
                    None => v,
                };
                self.line(format!("OtRt.set({a}, {i}, {v}, {loc});"));
            }
            _ => unreachable!("invalid assignment target"),
        }
    }

    fn has_effects(&self, e: &Expr) -> bool {
        !matches!(
            e.kind,
            ExprKind::Int(_)
                | ExprKind::Float(_)
                | ExprKind::Bool(_)
                | ExprKind::Str(_)
                | ExprKind::Local(_)
        )
    }

    /// Stores `code` in a fresh temporary unless it is trivially pure.
    fn spill(&mut self, code: &str, ty: &Type) -> String {
        if is_atom(code) {
            return code.to_string();
        }
        let t = self.tmp();
        let jt = self.jty(ty);
        self.line(format!("{jt} {t} = {code};"));
        t
    }

    /// Evaluates an expression for its side effects only.
    fn expr_stmt(&mut self, e: &Expr) {
        if self.dead {
            return;
        }
        if e.is_block_like() {
            self.expr_into(e, Target::Discard);
            return;
        }
        let code = self.expr(e);
        if self.dead || code.is_empty() {
            return;
        }
        if matches!(e.ty, Type::Void | Type::Never) {
            self.line(format!("{code};"));
        } else if !is_atom(&code) && !matches!(e.kind, ExprKind::FnRef(_)) {
            self.line(format!("OtRt.ignore({code});"));
        }
    }

    /// Lowers a block-like expression into statements, routing its value.
    fn expr_into(&mut self, e: &Expr, target: Target) {
        if self.dead {
            return;
        }
        match &e.kind {
            ExprKind::Block(b) => {
                self.line("{");
                let d = self.branch(|g| g.block_into(b, target));
                self.line("}");
                self.dead = d;
            }
            ExprKind::If { cond, then, els } => {
                let c = self.expr(cond);
                self.line(format!("if ({c}) {{"));
                let t1 = target.clone();
                let then_dead = self.branch(|g| g.block_into(then, t1));
                match els {
                    None => {
                        self.line("}");
                        self.dead = false;
                    }
                    Some(els) => {
                        let else_dead;
                        if let ExprKind::If { .. } = els.kind {
                            self.line("} else {");
                            let t2 = target.clone();
                            else_dead = self.branch(|g| g.expr_into(els, t2));
                        } else {
                            self.line("} else {");
                            let t2 = target.clone();
                            else_dead = self.branch(|g| g.branch_value(els, t2));
                        }
                        self.line("}");
                        self.dead = then_dead && else_dead;
                    }
                }
            }
            ExprKind::Match { scrut, arms } => {
                let s = self.expr(scrut);
                let s = self.spill(&s, &scrut.ty);
                let mut all_dead = true;
                let mut has_default = false;
                for (i, arm) in arms.iter().enumerate() {
                    let wildcard = arm.pats.iter().any(|p| matches!(p, Pat::Wildcard(_)));
                    let head = if wildcard {
                        has_default = true;
                        if i == 0 {
                            "{".to_string()
                        } else {
                            "} else {".to_string()
                        }
                    } else {
                        let conds: Vec<String> = arm
                            .pats
                            .iter()
                            .map(|p| self.pat_test(&s, &scrut.ty, p))
                            .collect();
                        let c = conds.join(" || ");
                        if i == 0 {
                            format!("if ({c}) {{")
                        } else {
                            format!("}} else if ({c}) {{")
                        }
                    };
                    self.line(head);
                    let t = target.clone();
                    let d = self.branch(|g| g.branch_value(&arm.body, t));
                    all_dead &= d;
                    if wildcard {
                        break;
                    }
                }
                if !has_default {
                    let needs_else =
                        e.ty.is_value() || e.ty.is_never() || !matches!(target, Target::Discard);
                    if needs_else && !arms.is_empty() {
                        self.line("} else {");
                        self.indent += 1;
                        self.line("throw OtRt.unreachable();");
                        self.indent -= 1;
                    } else {
                        all_dead = false;
                    }
                }
                if !arms.is_empty() {
                    self.line("}");
                }
                self.dead = all_dead && !arms.is_empty();
            }
            ExprKind::While { .. } | ExprKind::For { .. } | ExprKind::Loop(_) => {
                self.loop_stmt(e);
            }
            _ => unreachable!(),
        }
    }

    /// Value of a branch body (block or plain expression) routed to `target`.
    fn branch_value(&mut self, body: &Expr, target: Target) {
        if let ExprKind::Block(b) = &body.kind {
            self.block_into(b, target);
            return;
        }
        if body.is_block_like() {
            self.expr_into(body, target);
            return;
        }
        match target {
            Target::Discard => self.expr_stmt(body),
            Target::Assign(var) => {
                let v = self.expr(body);
                if !body.ty.is_never() {
                    self.line(format!("{var} = {v};"));
                }
            }
            Target::Return => {
                let v = self.expr(body);
                if !self.dead && !body.ty.is_never() {
                    let ty = self.ret_ty.clone();
                    self.emit_return(Some(v), &ty);
                }
            }
        }
    }

    fn pat_test(&mut self, s: &str, ty: &Type, p: &Pat) -> String {
        match p {
            Pat::Wildcard(_) => "true".into(),
            Pat::Int(n, _) => format!("{s} == {}", int_lit(*n)),
            Pat::Variant(_, _, _, idx) => format!("{s} == {idx}L"),
            Pat::Bool(b, _) => format!("{s} == {b}"),
            Pat::Str(lit, _) => format!("{s}.equals({})", java_str(lit)),
            Pat::Range(lo, hi, inc, _) => {
                let op = if *inc { "<=" } else { "<" };
                let _ = ty;
                format!("({s} >= {} && {s} {op} {})", int_lit(*lo), int_lit(*hi))
            }
        }
    }

    fn loop_stmt(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::While { cond, body } => {
                let saved = std::mem::take(&mut self.lines);
                let c = self.expr(cond);
                let prelude = std::mem::replace(&mut self.lines, saved);
                if prelude.is_empty() {
                    let c = if is_constant(cond) {
                        format!("OtRt.b({c})")
                    } else {
                        c
                    };
                    self.line(format!("while ({c}) {{"));
                    self.loop_body(body);
                } else {
                    self.line("while (true) {");
                    self.indent += 1;
                    for l in prelude {
                        self.lines.push(format!("    {l}"));
                    }
                    self.line(format!("if (!({c})) break;"));
                    self.indent -= 1;
                    self.loop_body(body);
                }
                self.line("}");
                self.dead = false;
            }
            ExprKind::For {
                local, iter, body, ..
            } => {
                let var = self.local_names[*local as usize].clone();
                match iter {
                    ForIter::Range {
                        start,
                        end,
                        inclusive,
                    } => {
                        let a = self.expr(start);
                        let a = self.spill(&a, &Type::Int);
                        let b = self.expr(end);
                        let b = self.spill(&b, &Type::Int);
                        if *inclusive {
                            let go = self.tmp();
                            self.line(format!(
                                "for (long {var} = {a}, {go} = ({a} <= {b} ? 1L : 0L); {go} != 0L; {go} = ({var} != {b} ? 1L : 0L), {var}++) {{"
                            ));
                        } else {
                            self.line(format!("for (long {var} = {a}; {var} < {b}; {var}++) {{"));
                        }
                        self.loop_body(body);
                    }
                    ForIter::Each(arr) => {
                        let a = self.expr(arr);
                        let a = self.spill_always(&a, &arr.ty);
                        let i = self.tmp();
                        let el = match &arr.ty {
                            Type::Array(el) => (**el).clone(),
                            _ => unreachable!(),
                        };
                        let jt = self.jty(&el);
                        self.line(format!("for (int {i} = 0; {i} < {a}.size(); {i}++) {{"));
                        self.indent += 1;
                        let read = self.unbox(format!("{a}.get({i})"), &el);
                        self.line(format!("{jt} {var} = {read};"));
                        self.indent -= 1;
                        self.loop_body(body);
                    }
                }
                self.line("}");
                self.dead = false;
            }
            ExprKind::Loop(body) => {
                self.line("while (true) {");
                self.loop_body(body);
                self.line("}");
                self.dead = e.ty.is_never();
            }
            _ => unreachable!(),
        }
    }

    fn spill_always(&mut self, code: &str, ty: &Type) -> String {
        if code.starts_with(|c: char| c.is_alphabetic())
            && code
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
        {
            return code.to_string();
        }
        let t = self.tmp();
        let jt = self.jty(ty);
        self.line(format!("{jt} {t} = {code};"));
        t
    }

    fn loop_body(&mut self, body: &Block) {
        self.loop_depths.push(self.defers.len());
        self.branch(|g| g.block_into(body, Target::Discard));
        self.loop_depths.pop();
    }

    // ── Expressions ──────────────────────────────────────────────────────────

    /// Generates several operands left to right, spilling earlier ones into
    /// temporaries if a later operand needs statements.
    fn operands(&mut self, es: &[&Expr]) -> Vec<String> {
        let mut ops: Vec<Operand> = Vec::new();
        for e in es {
            let saved = std::mem::take(&mut self.lines);
            let code = self.expr(e);
            let stmts = std::mem::replace(&mut self.lines, saved);
            ops.push(Operand {
                pure: !self.has_effects(e)
                    && matches!(
                        e.kind,
                        ExprKind::Int(_)
                            | ExprKind::Float(_)
                            | ExprKind::Bool(_)
                            | ExprKind::Str(_)
                    ),
                code,
                stmts,
            });
        }
        let mut out = Vec::new();
        for i in 0..ops.len() {
            let later_stmts = ops[i + 1..].iter().any(|o| !o.stmts.is_empty());
            for s in std::mem::take(&mut ops[i].stmts) {
                if !self.dead {
                    self.lines.push(s);
                }
            }
            if later_stmts && !ops[i].pure {
                let t = self.tmp();
                let jt = self.jty(&es[i].ty);
                if es[i].ty.is_value() {
                    self.line(format!("{jt} {t} = {};", ops[i].code));
                    out.push(t);
                    continue;
                }
            }
            out.push(ops[i].code.clone());
        }
        out
    }

    /// Evaluates a block-like expression into a temporary and returns it.
    fn lowered(&mut self, e: &Expr) -> String {
        if !e.ty.is_value() {
            self.expr_into(e, Target::Discard);
            return Self::default_value(&e.ty).to_string();
        }
        let t = self.tmp();
        let jt = self.jty(&e.ty);
        self.line(format!("{jt} {t} = {};", Self::default_value(&e.ty)));
        self.expr_into(e, Target::Assign(t.clone()));
        t
    }

    fn expr(&mut self, e: &Expr) -> String {
        if self.dead {
            return Self::default_value(&e.ty).to_string();
        }
        let span = e.span;
        match &e.kind {
            ExprKind::Int(n) => int_lit(*n),
            ExprKind::Float(f) => float_lit(*f),
            ExprKind::Bool(b) => b.to_string(),
            ExprKind::Str(s) => java_str(s),
            ExprKind::Interp(parts) => {
                let exprs: Vec<&Expr> = parts
                    .iter()
                    .filter_map(|p| {
                        if let InterpPart::Expr(x) = p {
                            Some(x)
                        } else {
                            None
                        }
                    })
                    .collect();
                let vals = self.operands(&exprs);
                let mut vi = vals.into_iter();
                let mut pieces = vec!["\"\"".to_string()];
                for p in parts {
                    match p {
                        InterpPart::Lit(s) => pieces.push(java_str(s)),
                        InterpPart::Expr(x) => {
                            let v = vi.next().unwrap();
                            pieces.push(self.to_str(&v, &x.ty));
                        }
                    }
                }
                format!("({})", pieces.join(" + "))
            }
            ExprKind::Local(id) => self.local_names[*id as usize].clone(),
            ExprKind::FnRef(sym) => format!("Main::{}", fn_name(sym)),
            ExprKind::EnumVariant { index, .. } => format!("{index}L"),
            ExprKind::Unary(op, inner) => {
                let v = self.expr(inner);
                match op {
                    UnOp::Neg => format!("(-({v}))"),
                    UnOp::Not => format!("(!({v}))"),
                    UnOp::BitNot => format!("(~({v}))"),
                }
            }
            ExprKind::Binary(op @ (BinOp::And | BinOp::Or), l, r) => {
                let lv = self.expr(l);
                let saved = std::mem::take(&mut self.lines);
                let saved_dead = self.dead;
                let rv = self.expr(r);
                let rstmts = std::mem::replace(&mut self.lines, saved);
                self.dead = saved_dead;
                if rstmts.is_empty() {
                    format!("({lv} {} {rv})", op.symbol())
                } else {
                    let t = self.tmp();
                    self.line(format!("boolean {t} = {lv};"));
                    let c = if *op == BinOp::And {
                        t.clone()
                    } else {
                        format!("!{t}")
                    };
                    self.line(format!("if ({c}) {{"));
                    self.branch(|g| {
                        let rv = g.expr(r);
                        g.line(format!("{t} = {rv};"));
                    });
                    self.line("}");
                    t
                }
            }
            ExprKind::Binary(op, l, r) => {
                let v = self.operands(&[l, r]);
                self.binop(*op, &l.ty, &v[0], &v[1], span)
            }
            ExprKind::Cast(inner, _) => {
                let v = self.expr(inner);
                match (&inner.ty, &e.ty) {
                    (Type::Int, Type::Float) => format!("((double) {v})"),
                    (Type::Float, Type::Int) => format!("((long) {v})"),
                    (Type::Bool, Type::Int) => format!("({v} ? 1L : 0L)"),
                    _ => v,
                }
            }
            ExprKind::CallFn { symbol, args } => {
                let refs: Vec<&Expr> = args.iter().collect();
                let v = self.operands(&refs);
                format!("{}({})", fn_name(symbol), v.join(", "))
            }
            ExprKind::CallExtern { name, .. } => {
                panic!("extern function '{name}' reached JVM codegen (rejected earlier)");
            }
            ExprKind::CallIndirect { callee, args } => {
                let mut refs: Vec<&Expr> = vec![callee];
                refs.extend(args.iter());
                let v = self.operands(&refs);
                let f = if let ExprKind::FnRef(_) = callee.kind {
                    let it = self.jty(&callee.ty);
                    format!("(({it}) {})", v[0])
                } else {
                    v[0].clone()
                };
                format!("{f}.call({})", v[1..].join(", "))
            }
            ExprKind::CallBuiltin { builtin, args } => self.builtin(*builtin, args, e),
            ExprKind::FieldIdx {
                obj,
                struct_name,
                index,
            } => {
                let o = self.expr(obj);
                let f = ident(&self.m.struct_info(struct_name).fields[*index as usize].0);
                format!("{o}.{f}")
            }
            ExprKind::Index { obj, idx } => {
                let v = self.operands(&[obj, idx]);
                let loc = self.loc(span);
                self.unbox(format!("OtRt.get({}, {}, {loc})", v[0], v[1]), &e.ty)
            }
            ExprKind::New {
                struct_name,
                fields,
            } => {
                let refs: Vec<&Expr> = fields.iter().collect();
                let v = self.operands(&refs);
                format!("new S_{struct_name}({})", v.join(", "))
            }
            ExprKind::ArrayLit(items) => {
                let el = match &e.ty {
                    Type::Array(el) => (**el).clone(),
                    _ => unreachable!(),
                };
                let b = self.boxed(&el);
                if items.is_empty() {
                    return format!("new ArrayList<{b}>()");
                }
                let refs: Vec<&Expr> = items.iter().collect();
                let v = self.operands(&refs);
                format!("OtRt.<{b}>list({})", v.join(", "))
            }
            ExprKind::ArrayRepeat { value, count } => {
                let el = match &e.ty {
                    Type::Array(el) => (**el).clone(),
                    _ => unreachable!(),
                };
                let b = self.boxed(&el);
                let v = self.operands(&[value, count]);
                let loc = self.loc(span);
                format!("OtRt.<{b}>repeat({}, {}, {loc})", v[0], v[1])
            }
            ExprKind::If { .. }
            | ExprKind::Match { .. }
            | ExprKind::Block(_)
            | ExprKind::While { .. }
            | ExprKind::For { .. }
            | ExprKind::Loop(_) => self.lowered(e),
            ExprKind::Break | ExprKind::Continue => {
                let depth = *self.loop_depths.last().expect("break outside loop");
                self.emit_defers(depth);
                self.line(if matches!(e.kind, ExprKind::Break) {
                    "break;"
                } else {
                    "continue;"
                });
                self.dead = true;
                String::new()
            }
            ExprKind::Return(v) => {
                match v {
                    Some(v) if v.ty.is_value() => {
                        let x = self.expr(v);
                        if !self.dead {
                            self.emit_return(Some(x), &v.ty.clone());
                        }
                    }
                    Some(v) => {
                        self.expr_stmt(v);
                        if !self.dead {
                            self.emit_return(None, &Type::Void);
                        }
                    }
                    None => self.emit_return(None, &Type::Void),
                }
                String::new()
            }
            other => unreachable!("unresolved expression reached codegen: {other:?}"),
        }
    }

    fn binop(&mut self, op: BinOp, ty: &Type, l: &str, r: &str, span: Span) -> String {
        use BinOp::*;
        match (ty, op) {
            (Type::Str, Add) => format!("({l} + {r})"),
            (Type::Str, Eq) => format!("{l}.equals({r})"),
            (Type::Str, Ne) => format!("(!{l}.equals({r}))"),
            (Type::Str, _) => format!("(OtRt.cmp({l}, {r}) {} 0)", op.symbol()),
            (Type::Float, Pow) => format!("Math.pow({l}, {r})"),
            (Type::Int, Div) => format!("OtRt.div({l}, {r}, {})", self.loc(span)),
            (Type::Int, Rem) => format!("OtRt.rem({l}, {r}, {})", self.loc(span)),
            (Type::Int, Pow) => format!("OtRt.pow({l}, {r}, {})", self.loc(span)),
            _ => format!("({l} {} {r})", op.symbol()),
        }
    }

    #[allow(clippy::wrong_self_convention)]
    fn to_str(&mut self, v: &str, ty: &Type) -> String {
        match ty {
            Type::Str => v.to_string(),
            Type::Int => format!("Long.toString({v})"),
            Type::Float => format!("OtRt.fstr({v})"),
            Type::Bool => format!("Boolean.toString({v})"),
            Type::Enum(name) => format!("E_{name}[(int) ({v})]"),
            _ => unreachable!("not printable"),
        }
    }

    fn builtin(&mut self, b: Builtin, args: &[Expr], e: &Expr) -> String {
        use Builtin::*;
        let span = e.span;
        let refs: Vec<&Expr> = args.iter().collect();
        match b {
            Print | Println | Eprintln => {
                let flags = (if b == Print { 0 } else { 1 }) | (if b == Eprintln { 2 } else { 0 });
                match args.first() {
                    None => format!("OtRt.print(\"\", {flags}L)"),
                    Some(a) => {
                        let v = self.operands(&refs)[0].clone();
                        match &a.ty {
                            Type::Int | Type::Float | Type::Str => {
                                format!("OtRt.print({v}, {flags}L)")
                            }
                            t => {
                                let s = self.to_str(&v, t);
                                format!("OtRt.print({s}, {flags}L)")
                            }
                        }
                    }
                }
            }
            Assert => {
                let v = self.operands(&refs);
                let msg = v.get(1).cloned().unwrap_or_else(|| "null".into());
                let loc = self.loc(span);
                self.line(format!(
                    "if (!({})) throw OtRt.assertFail({msg}, {loc});",
                    v[0]
                ));
                String::new()
            }
            Exit => {
                let v = self.operands(&refs);
                self.line(format!("throw OtRt.exit({});", v[0]));
                self.dead = true;
                String::new()
            }
            Panic => {
                let v = self.operands(&refs);
                self.line(format!("throw OtRt.panic({});", v[0]));
                self.dead = true;
                String::new()
            }
            ReadLine => "OtRt.readLine()".into(),
            Clock => "OtRt.clock()".into(),
            ParseInt | ParseFloat | Chr => {
                let v = self.operands(&refs);
                let loc = self.loc(span);
                let f = match b {
                    ParseInt => "parseInt",
                    ParseFloat => "parseFloat",
                    _ => "chr",
                };
                format!("OtRt.{f}({}, {loc})", v[0])
            }
            Sqrt | Sin | Cos | Tan | Exp | Ln | Floor | Ceil => {
                let v = self.operands(&refs);
                let f = match b {
                    Sqrt => "Math.sqrt",
                    Sin => "StrictMath.sin",
                    Cos => "StrictMath.cos",
                    Tan => "StrictMath.tan",
                    Exp => "StrictMath.exp",
                    Ln => "StrictMath.log",
                    Floor => "Math.floor",
                    _ => "Math.ceil",
                };
                format!("{f}({})", v[0])
            }
            Round => {
                let v = self.operands(&refs);
                format!("OtRt.round({})", v[0])
            }
            Atan2 => {
                let v = self.operands(&refs);
                format!("StrictMath.atan2({}, {})", v[0], v[1])
            }
            Abs => {
                let v = self.operands(&refs);
                format!("Math.abs({})", v[0])
            }
            Min | Max => {
                let v = self.operands(&refs);
                let f = if b == Min { "min" } else { "max" };
                format!("OtRt.{f}({}, {})", v[0], v[1])
            }
            ArrLen => {
                let v = self.operands(&refs);
                format!("((long) {}.size())", v[0])
            }
            ArrPush => {
                let v = self.operands(&refs);
                format!("{}.add({})", v[0], v[1])
            }
            ArrPop => {
                let v = self.operands(&refs);
                let loc = self.loc(span);
                self.unbox(format!("OtRt.pop({}, {loc})", v[0]), &e.ty)
            }
            StrLen => {
                let v = self.operands(&refs);
                format!("OtRt.len({})", v[0])
            }
            StrByte => {
                let v = self.operands(&refs);
                let loc = self.loc(span);
                format!("OtRt.byteAt({}, {}, {loc})", v[0], v[1])
            }
            StrSlice => {
                let v = self.operands(&refs);
                let loc = self.loc(span);
                format!("OtRt.slice({}, {}, {}, {loc})", v[0], v[1], v[2])
            }
            StrContains => {
                let v = self.operands(&refs);
                format!("{}.contains({})", v[0], v[1])
            }
            StrFind => {
                let v = self.operands(&refs);
                format!("OtRt.find({}, {})", v[0], v[1])
            }
        }
    }
}

/// A Java expression that can be repeated without changing behaviour.
fn is_atom(code: &str) -> bool {
    if code.is_empty() {
        return true;
    }
    let simple_ident = code
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '$');
    let literal =
        code.starts_with('"') && code.ends_with('"') && !code[1..code.len() - 1].contains('"');
    let number = code.trim_end_matches('L').parse::<f64>().is_ok();
    (simple_ident && !code.starts_with(|c: char| c.is_ascii_digit()) && code.starts_with("t$"))
        || literal
        || number
        || code == "true"
        || code == "false"
        || code == "null"
}

/// Whether an expression is a compile-time constant in Java's eyes (javac
/// treats `while (true)` specially, so such conditions are wrapped).
fn is_constant(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::Str(_) => true,
        ExprKind::Unary(_, x) | ExprKind::Cast(x, _) => is_constant(x),
        ExprKind::Binary(_, l, r) => is_constant(l) && is_constant(r),
        _ => false,
    }
}
