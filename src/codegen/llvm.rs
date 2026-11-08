//! LLVM back end: emits textual LLVM IR, which clang compiles together with
//! the C runtime (`runtime/orbitron_rt.c`) into a native executable.

use std::collections::HashMap;
use std::fmt::Write;

use crate::ast::*;
use crate::check::Module;
use crate::diag::Span;
use crate::types::Type;

const PRELUDE: &str = r#"
declare ptr @ot_alloc(i64)
declare void @ot_panic(ptr) noreturn
declare void @ot_exit(i64) noreturn
declare void @ot_assert_fail(ptr, ptr) noreturn
declare i64 @ot_idiv(i64, i64, ptr)
declare i64 @ot_irem(i64, i64, ptr)
declare i64 @ot_ipow(i64, i64, ptr)
declare i64 @ot_f2i(double)
declare double @ot_clock()
declare ptr @ot_str_concat(ptr, ptr)
declare i64 @ot_str_eq(ptr, ptr)
declare i64 @ot_str_cmp(ptr, ptr)
declare i64 @ot_str_len(ptr)
declare i64 @ot_str_byte(ptr, i64, ptr)
declare ptr @ot_str_slice(ptr, i64, i64, ptr)
declare i64 @ot_str_contains(ptr, ptr)
declare i64 @ot_str_find(ptr, ptr)
declare ptr @ot_chr(i64, ptr)
declare ptr @ot_int_to_str(i64)
declare ptr @ot_float_to_str(double)
declare void @ot_print_str(ptr, i64)
declare void @ot_print_int(i64, i64)
declare void @ot_print_float(double, i64)
declare ptr @ot_read_line()
declare i64 @ot_parse_int(ptr, ptr)
declare double @ot_parse_float(ptr, ptr)
declare ptr @ot_arr_new(i64, ptr)
declare i64 @ot_arr_len(ptr)
declare ptr @ot_arr_slot(ptr, i64, ptr)
declare ptr @ot_arr_data(ptr)
declare ptr @ot_arr_push_slot(ptr)
declare ptr @ot_arr_pop_slot(ptr, ptr)
declare double @pow(double, double)
declare double @sqrt(double)
declare double @sin(double)
declare double @cos(double)
declare double @tan(double)
declare double @atan2(double, double)
declare double @exp(double)
declare double @log(double)
declare double @floor(double)
declare double @ceil(double)
declare double @round(double)
declare double @fabs(double)
"#;

const LIBC_NAMES: &[&str] = &[
    "pow", "sqrt", "sin", "cos", "tan", "atan2", "exp", "log", "floor", "ceil", "round", "fabs",
];

struct LoopLabels {
    brk: String,
    cont: String,
    defer_depth: usize,
}

pub struct LlvmGen<'m> {
    m: &'m Module,
    files: &'m [String],
    globals: String,
    strings: HashMap<String, String>,
    out: String,
    // per-function state
    body: String,
    allocas: String,
    tmp: usize,
    labels: usize,
    terminated: bool,
    locals: Vec<String>,
    local_tys: Vec<Type>,
    loops: Vec<LoopLabels>,
    defers: Vec<Vec<Expr>>,
}

pub fn emit(m: &Module, files: &[String]) -> String {
    let mut g = LlvmGen {
        m,
        files,
        globals: String::new(),
        strings: HashMap::new(),
        out: String::new(),
        body: String::new(),
        allocas: String::new(),
        tmp: 0,
        labels: 0,
        terminated: false,
        locals: Vec::new(),
        local_tys: Vec::new(),
        loops: Vec::new(),
        defers: Vec::new(),
    };
    g.run()
}

fn lty(t: &Type) -> &'static str {
    match t {
        Type::Int | Type::Enum(_) => "i64",
        Type::Float => "double",
        Type::Bool => "i1",
        Type::Void | Type::Never => "void",
        Type::Str | Type::Array(_) | Type::Struct(_) | Type::Fn(..) => "ptr",
        Type::Unknown | Type::Error => panic!("unresolved type in codegen"),
    }
}

fn fn_name(symbol: &str) -> String {
    format!("@\"ot.{symbol}\"")
}

fn struct_ty(name: &str) -> String {
    format!("%\"st.{name}\"")
}

fn float_lit(f: f64) -> String {
    format!("0x{:016X}", f.to_bits())
}

impl<'m> LlvmGen<'m> {
    fn run(&mut self) -> String {
        let mut head = String::new();
        head.push_str("; Сгенерировано компилятором Orbitron\n");
        for s in &self.m.structs {
            let fields: Vec<&str> = s.fields.iter().map(|(_, t)| lty(t)).collect();
            let _ = writeln!(
                head,
                "{} = type {{ {} }}",
                struct_ty(&s.name),
                fields.join(", ")
            );
        }
        for en in &self.m.enums {
            let names: Vec<String> = en
                .variants
                .iter()
                .map(|v| format!("ptr {}", self.str_const(v)))
                .collect();
            let _ = writeln!(
                self.globals,
                "@\"enum.{}\" = private constant [{} x ptr] [{}]",
                en.name,
                names.len(),
                names.join(", ")
            );
        }
        head.push_str(PRELUDE);
        for ex in &self.m.externs {
            if LIBC_NAMES.contains(&ex.name.as_str()) {
                continue;
            }
            let ps: Vec<String> = ex.params.iter().map(extern_param).collect();
            let ret = if ex.ret == Type::Bool {
                "zeroext i1".to_string()
            } else {
                lty(&ex.ret).to_string()
            };
            let _ = writeln!(head, "declare {} @{}({})", ret, ex.name, ps.join(", "));
        }

        for f in &self.m.fns {
            self.function(f);
        }

        let main = self.m.fns.iter().find(|f| f.symbol == "main");
        if let Some(main) = main {
            self.out
                .push_str("define i32 @main(i32 %argc, ptr %argv) {\nentry:\n");
            if main.ret_ty == Type::Int {
                let _ = writeln!(self.out, "  %r = call i64 {}()", fn_name("main"));
                self.out
                    .push_str("  call void @ot_exit(i64 %r)\n  unreachable\n}\n");
            } else {
                let _ = writeln!(self.out, "  call void {}()", fn_name("main"));
                self.out
                    .push_str("  call void @ot_exit(i64 0)\n  unreachable\n}\n");
            }
        }

        format!("{head}\n{}\n{}", self.globals, self.out)
    }

    // ── Emission helpers ─────────────────────────────────────────────────────

    fn tmp(&mut self) -> String {
        self.tmp += 1;
        format!("%t{}", self.tmp)
    }

    fn label(&mut self, name: &str) -> String {
        self.labels += 1;
        format!("{name}.{}", self.labels)
    }

    fn emit(&mut self, line: impl AsRef<str>) {
        if self.terminated {
            let l = self.label("dead");
            self.body.push_str(&format!("{l}:\n"));
            self.terminated = false;
        }
        self.body.push_str("  ");
        self.body.push_str(line.as_ref());
        self.body.push('\n');
    }

    fn term(&mut self, line: impl AsRef<str>) {
        self.emit(line);
        self.terminated = true;
    }

    /// Branch to `l` unless the current block already ended.
    fn br(&mut self, l: &str) {
        if !self.terminated {
            self.term(format!("br label %{l}"));
        }
    }

    fn start_block(&mut self, l: &str) {
        self.br(l);
        self.body.push_str(&format!("{l}:\n"));
        self.terminated = false;
    }

    fn alloca(&mut self, ty: &str) -> String {
        let t = self.tmp();
        let _ = writeln!(self.allocas, "  {t} = alloca {ty}");
        t
    }

    fn str_const(&mut self, s: &str) -> String {
        if let Some(n) = self.strings.get(s) {
            return n.clone();
        }
        let name = format!("@.s.{}", self.strings.len());
        let mut enc = String::new();
        for b in s.bytes() {
            if (0x20..0x7f).contains(&b) && b != b'"' && b != b'\\' {
                enc.push(b as char);
            } else {
                let _ = write!(enc, "\\{b:02X}");
            }
        }
        let _ = writeln!(
            self.globals,
            "{name} = private unnamed_addr constant [{} x i8] c\"{enc}\\00\"",
            s.len() + 1
        );
        self.strings.insert(s.to_string(), name.clone());
        name
    }

    fn loc(&mut self, span: Span) -> String {
        let file = self
            .files
            .get(span.file as usize)
            .map(|s| s.as_str())
            .unwrap_or("?");
        let text = format!("{}:{}:{}", file, span.line, span.col);
        self.str_const(&text)
    }

    fn call(&mut self, ret: &str, func: &str, args: &[(&str, &str)]) -> String {
        let a: Vec<String> = args.iter().map(|(t, v)| format!("{t} {v}")).collect();
        if ret == "void" {
            self.emit(format!("call void {func}({})", a.join(", ")));
            String::new()
        } else {
            let t = self.tmp();
            self.emit(format!("{t} = call {ret} {func}({})", a.join(", ")));
            t
        }
    }

    fn call_bool(&mut self, func: &str, args: &[(&str, &str)]) -> String {
        let r = self.call("i64", func, args);
        let t = self.tmp();
        self.emit(format!("{t} = icmp ne i64 {r}, 0"));
        t
    }

    // ── Functions ────────────────────────────────────────────────────────────

    fn function(&mut self, f: &FnDecl) {
        self.body.clear();
        self.allocas.clear();
        self.tmp = 0;
        self.labels = 0;
        self.terminated = false;
        self.loops.clear();
        self.defers.clear();
        self.locals.clear();
        self.local_tys.clear();

        let n_params = f.params.len() + usize::from(f.has_self);
        let mut sig = Vec::new();
        for (i, l) in f.locals.iter().enumerate() {
            let name = format!("%l{i}");
            if l.ty.is_value() {
                let _ = writeln!(self.allocas, "  {name} = alloca {}", lty(&l.ty));
            }
            if i < n_params {
                sig.push(format!("{} %p{i}", lty(&l.ty)));
                let _ = writeln!(self.allocas, "  store {} %p{i}, ptr {name}", lty(&l.ty));
            }
            self.locals.push(name);
            self.local_tys.push(l.ty.clone());
        }

        let tail = self.block(&f.body);
        if !self.terminated {
            match (&f.ret_ty, tail) {
                (Type::Void, _) => self.term("ret void"),
                (t, Some(v)) => self.term(format!("ret {} {v}", lty(t))),
                (_, None) => self.term("unreachable"),
            }
        }

        let _ = writeln!(
            self.out,
            "define internal {} {}({}) {{\nentry:\n{}{}}}\n",
            lty(&f.ret_ty),
            fn_name(&f.symbol),
            sig.join(", "),
            self.allocas,
            self.body
        );
    }

    fn emit_defers(&mut self, from_depth: usize) {
        let pending: Vec<Expr> = self.defers[from_depth..]
            .iter()
            .rev()
            .flat_map(|scope| scope.iter().rev().cloned())
            .collect();
        let saved = std::mem::take(&mut self.defers);
        for d in &pending {
            self.expr(d);
        }
        self.defers = saved;
    }

    fn block(&mut self, b: &Block) -> Option<String> {
        self.defers.push(Vec::new());
        for s in &b.stmts {
            self.stmt(s);
        }
        let v = b.tail.as_ref().and_then(|t| self.expr(t));
        let depth = self.defers.len() - 1;
        if !self.terminated && !self.defers[depth].is_empty() {
            self.emit_defers(depth);
        }
        self.defers.pop();
        v
    }

    fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Let { init, local, .. } => {
                if let Some(v) = self.expr(init) {
                    let ty = lty(&self.local_tys[*local as usize]);
                    let p = self.locals[*local as usize].clone();
                    self.emit(format!("store {ty} {v}, ptr {p}"));
                }
            }
            StmtKind::Assign { target, op, value } => self.assign(target, *op, value, s.span),
            StmtKind::Expr { expr, .. } => {
                self.expr(expr);
            }
            StmtKind::Defer(e) => self.defers.last_mut().unwrap().push(e.clone()),
        }
    }

    fn assign(&mut self, target: &Expr, op: Option<BinOp>, value: &Expr, span: Span) {
        let ty = lty(&target.ty);
        // Address of the target.
        let (addr, old_needed) = match &target.kind {
            ExprKind::Local(id) => (self.locals[*id as usize].clone(), op.is_some()),
            ExprKind::FieldIdx {
                obj,
                struct_name,
                index,
            } => {
                let Some(p) = self.expr(obj) else { return };
                let a = self.tmp();
                self.emit(format!(
                    "{a} = getelementptr {}, ptr {p}, i32 0, i32 {index}",
                    struct_ty(struct_name)
                ));
                (a, op.is_some())
            }
            ExprKind::Index { obj, idx } => {
                let Some(arr) = self.expr(obj) else { return };
                let Some(i) = self.expr(idx) else { return };
                let loc = self.loc(target.span);
                if op.is_none() {
                    // Evaluate the value before the bounds check (same order as the JVM back end).
                    let Some(v) = self.expr(value) else { return };
                    let slot = self.call(
                        "ptr",
                        "@ot_arr_slot",
                        &[("ptr", &arr), ("i64", &i), ("ptr", &loc)],
                    );
                    self.emit(format!("store {ty} {v}, ptr {slot}"));
                    return;
                }
                let slot = self.call(
                    "ptr",
                    "@ot_arr_slot",
                    &[("ptr", &arr), ("i64", &i), ("ptr", &loc)],
                );
                (slot, true)
            }
            _ => unreachable!("invalid assignment target"),
        };
        let new = if old_needed {
            let old = self.tmp();
            self.emit(format!("{old} = load {ty}, ptr {addr}"));
            let Some(v) = self.expr(value) else { return };
            self.binop(op.unwrap(), &target.ty, &old, &v, span)
        } else {
            let Some(v) = self.expr(value) else { return };
            v
        };
        self.emit(format!("store {ty} {new}, ptr {addr}"));
    }

    // ── Expressions ──────────────────────────────────────────────────────────

    fn expr(&mut self, e: &Expr) -> Option<String> {
        let span = e.span;
        match &e.kind {
            ExprKind::Int(n) => Some(n.to_string()),
            ExprKind::Float(f) => Some(float_lit(*f)),
            ExprKind::Bool(b) => Some(b.to_string()),
            ExprKind::Str(s) => Some(self.str_const(s)),
            ExprKind::Interp(parts) => {
                let mut acc: Option<String> = None;
                for p in parts {
                    let piece = match p {
                        InterpPart::Lit(s) => self.str_const(s),
                        InterpPart::Expr(x) => {
                            let v = self.expr(x)?;
                            self.to_str(&v, &x.ty)
                        }
                    };
                    acc = Some(match acc {
                        None => piece,
                        Some(a) => {
                            self.call("ptr", "@ot_str_concat", &[("ptr", &a), ("ptr", &piece)])
                        }
                    });
                }
                Some(acc.unwrap_or_else(|| self.str_const("")))
            }
            ExprKind::Local(id) => {
                let t = self.tmp();
                let ty = lty(&self.local_tys[*id as usize]);
                let p = self.locals[*id as usize].clone();
                self.emit(format!("{t} = load {ty}, ptr {p}"));
                Some(t)
            }
            ExprKind::FnRef(sym) => Some(fn_name(sym)),
            ExprKind::EnumVariant { index, .. } => Some(index.to_string()),
            ExprKind::Unary(op, inner) => {
                let v = self.expr(inner)?;
                let t = self.tmp();
                match (op, &inner.ty) {
                    (UnOp::Neg, Type::Float) => self.emit(format!("{t} = fneg double {v}")),
                    (UnOp::Neg, _) => self.emit(format!("{t} = sub i64 0, {v}")),
                    (UnOp::Not, _) => self.emit(format!("{t} = xor i1 {v}, true")),
                    (UnOp::BitNot, _) => self.emit(format!("{t} = xor i64 {v}, -1")),
                }
                Some(t)
            }
            ExprKind::Binary(op @ (BinOp::And | BinOp::Or), l, r) => {
                let res = self.alloca("i1");
                let lv = self.expr(l)?;
                self.emit(format!("store i1 {lv}, ptr {res}"));
                let rhs_l = self.label("logic.rhs");
                let end_l = self.label("logic.end");
                if *op == BinOp::And {
                    self.term(format!("br i1 {lv}, label %{rhs_l}, label %{end_l}"));
                } else {
                    self.term(format!("br i1 {lv}, label %{end_l}, label %{rhs_l}"));
                }
                self.start_block(&rhs_l);
                if let Some(rv) = self.expr(r) {
                    self.emit(format!("store i1 {rv}, ptr {res}"));
                }
                self.start_block(&end_l);
                let t = self.tmp();
                self.emit(format!("{t} = load i1, ptr {res}"));
                Some(t)
            }
            ExprKind::Binary(op, l, r) => {
                let lv = self.expr(l)?;
                let rv = self.expr(r)?;
                Some(self.binop(*op, &l.ty, &lv, &rv, span))
            }
            ExprKind::Cast(inner, _) => {
                let v = self.expr(inner)?;
                Some(match (&inner.ty, &e.ty) {
                    (Type::Int, Type::Float) => {
                        let t = self.tmp();
                        self.emit(format!("{t} = sitofp i64 {v} to double"));
                        t
                    }
                    (Type::Float, Type::Int) => self.call("i64", "@ot_f2i", &[("double", &v)]),
                    (Type::Bool, Type::Int) => {
                        let t = self.tmp();
                        self.emit(format!("{t} = zext i1 {v} to i64"));
                        t
                    }
                    _ => v,
                })
            }
            ExprKind::CallFn { symbol, args } => {
                let vals = self.args(args)?;
                let ret = lty(&e.ty);
                let a: Vec<(&str, &str)> = args
                    .iter()
                    .zip(&vals)
                    .map(|(a, v)| (lty(&a.ty), v.as_str()))
                    .collect();
                let r = self.call(ret, &fn_name(symbol), &a);
                self.never_check(&e.ty);
                if ret == "void" {
                    None
                } else {
                    Some(r)
                }
            }
            ExprKind::CallExtern { name, args } => {
                let vals = self.args(args)?;
                let ret_ty = &e.ty;
                let ps: Vec<String> = args
                    .iter()
                    .zip(&vals)
                    .map(|(a, v)| format!("{} {v}", extern_param(&a.ty)))
                    .collect();
                let ret = if *ret_ty == Type::Bool {
                    "zeroext i1"
                } else {
                    lty(ret_ty)
                };
                if *ret_ty == Type::Void {
                    self.emit(format!("call void @{name}({})", ps.join(", ")));
                    None
                } else {
                    let t = self.tmp();
                    self.emit(format!("{t} = call {ret} @{name}({})", ps.join(", ")));
                    Some(t)
                }
            }
            ExprKind::CallIndirect { callee, args } => {
                let f = self.expr(callee)?;
                let vals = self.args(args)?;
                let ret = lty(&e.ty);
                let a: Vec<(&str, &str)> = args
                    .iter()
                    .zip(&vals)
                    .map(|(a, v)| (lty(&a.ty), v.as_str()))
                    .collect();
                let r = self.call(ret, &f, &a);
                if ret == "void" {
                    None
                } else {
                    Some(r)
                }
            }
            ExprKind::CallBuiltin { builtin, args } => self.builtin(*builtin, args, e),
            ExprKind::FieldIdx {
                obj,
                struct_name,
                index,
            } => {
                let p = self.expr(obj)?;
                let a = self.tmp();
                self.emit(format!(
                    "{a} = getelementptr {}, ptr {p}, i32 0, i32 {index}",
                    struct_ty(struct_name)
                ));
                let t = self.tmp();
                self.emit(format!("{t} = load {}, ptr {a}", lty(&e.ty)));
                Some(t)
            }
            ExprKind::Index { obj, idx } => {
                let arr = self.expr(obj)?;
                let i = self.expr(idx)?;
                let loc = self.loc(span);
                let slot = self.call(
                    "ptr",
                    "@ot_arr_slot",
                    &[("ptr", &arr), ("i64", &i), ("ptr", &loc)],
                );
                let t = self.tmp();
                self.emit(format!("{t} = load {}, ptr {slot}", lty(&e.ty)));
                Some(t)
            }
            ExprKind::New {
                struct_name,
                fields,
            } => {
                let vals = self.args(fields)?;
                let st = struct_ty(struct_name);
                let szp = self.tmp();
                self.emit(format!("{szp} = getelementptr {st}, ptr null, i32 1"));
                let sz = self.tmp();
                self.emit(format!("{sz} = ptrtoint ptr {szp} to i64"));
                let p = self.call("ptr", "@ot_alloc", &[("i64", &sz)]);
                for (i, (f, v)) in fields.iter().zip(&vals).enumerate() {
                    let a = self.tmp();
                    self.emit(format!("{a} = getelementptr {st}, ptr {p}, i32 0, i32 {i}"));
                    self.emit(format!("store {} {v}, ptr {a}", lty(&f.ty)));
                }
                Some(p)
            }
            ExprKind::ArrayLit(items) => {
                let vals = self.args(items)?;
                let n = items.len().to_string();
                let arr = self.call("ptr", "@ot_arr_new", &[("i64", &n), ("ptr", "null")]);
                if !items.is_empty() {
                    let data = self.call("ptr", "@ot_arr_data", &[("ptr", &arr)]);
                    for (i, (it, v)) in items.iter().zip(&vals).enumerate() {
                        let a = self.tmp();
                        self.emit(format!("{a} = getelementptr i64, ptr {data}, i64 {i}"));
                        self.emit(format!("store {} {v}, ptr {a}", lty(&it.ty)));
                    }
                }
                Some(arr)
            }
            ExprKind::ArrayRepeat { value, count } => {
                let v = self.expr(value)?;
                let n = self.expr(count)?;
                let loc = self.loc(span);
                let arr = self.call("ptr", "@ot_arr_new", &[("i64", &n), ("ptr", &loc)]);
                let data = self.call("ptr", "@ot_arr_data", &[("ptr", &arr)]);
                let i = self.alloca("i64");
                self.emit(format!("store i64 0, ptr {i}"));
                let cond = self.label("fill.cond");
                let body = self.label("fill.body");
                let end = self.label("fill.end");
                self.start_block(&cond);
                let iv = self.tmp();
                self.emit(format!("{iv} = load i64, ptr {i}"));
                let c = self.tmp();
                self.emit(format!("{c} = icmp slt i64 {iv}, {n}"));
                self.term(format!("br i1 {c}, label %{body}, label %{end}"));
                self.start_block(&body);
                let a = self.tmp();
                self.emit(format!("{a} = getelementptr i64, ptr {data}, i64 {iv}"));
                self.emit(format!("store {} {v}, ptr {a}", lty(&value.ty)));
                let nx = self.tmp();
                self.emit(format!("{nx} = add i64 {iv}, 1"));
                self.emit(format!("store i64 {nx}, ptr {i}"));
                self.br(&cond);
                self.start_block(&end);
                Some(arr)
            }
            ExprKind::If { cond, then, els } => {
                let res = if e.ty.is_value() {
                    Some(self.alloca(lty(&e.ty)))
                } else {
                    None
                };
                let c = self.expr(cond)?;
                let then_l = self.label("if.then");
                let else_l = self.label("if.else");
                let end_l = self.label("if.end");
                let target = if els.is_some() { &else_l } else { &end_l };
                self.term(format!("br i1 {c}, label %{then_l}, label %{target}"));
                self.start_block(&then_l);
                let tv = self.block(then);
                self.store_result(&res, &e.ty, tv);
                self.br(&end_l);
                if let Some(els) = els {
                    self.start_block(&else_l);
                    let ev = self.expr(els);
                    self.store_result(&res, &e.ty, ev);
                    self.br(&end_l);
                }
                self.start_block(&end_l);
                self.load_result(&res, &e.ty)
            }
            ExprKind::Match { scrut, arms } => self.match_expr(e, scrut, arms),
            ExprKind::Block(b) => self.block(b),
            ExprKind::While { cond, body } => {
                let cond_l = self.label("while.cond");
                let body_l = self.label("while.body");
                let end_l = self.label("while.end");
                self.start_block(&cond_l);
                if let Some(c) = self.expr(cond) {
                    self.term(format!("br i1 {c}, label %{body_l}, label %{end_l}"));
                }
                self.start_block(&body_l);
                self.loops.push(LoopLabels {
                    brk: end_l.clone(),
                    cont: cond_l.clone(),
                    defer_depth: self.defers.len(),
                });
                self.block(body);
                self.loops.pop();
                self.br(&cond_l);
                self.start_block(&end_l);
                None
            }
            ExprKind::For {
                local, iter, body, ..
            } => {
                self.for_loop(*local, iter, body);
                None
            }
            ExprKind::Loop(body) => {
                let body_l = self.label("loop.body");
                let end_l = self.label("loop.end");
                self.start_block(&body_l);
                self.loops.push(LoopLabels {
                    brk: end_l.clone(),
                    cont: body_l.clone(),
                    defer_depth: self.defers.len(),
                });
                self.block(body);
                self.loops.pop();
                self.br(&body_l);
                self.start_block(&end_l);
                if e.ty.is_never() {
                    self.term("unreachable");
                }
                None
            }
            ExprKind::Break | ExprKind::Continue => {
                let l = self.loops.last().expect("break outside loop");
                let (target, depth) = if matches!(e.kind, ExprKind::Break) {
                    (l.brk.clone(), l.defer_depth)
                } else {
                    (l.cont.clone(), l.defer_depth)
                };
                self.emit_defers(depth);
                self.term(format!("br label %{target}"));
                None
            }
            ExprKind::Return(v) => {
                let val = match v {
                    Some(v) => match self.expr(v) {
                        Some(x) => Some((lty(&v.ty), x)),
                        None if v.ty.is_never() => return None,
                        None => None,
                    },
                    None => None,
                };
                self.emit_defers(0);
                match val {
                    Some((ty, x)) => self.term(format!("ret {ty} {x}")),
                    None => self.term("ret void"),
                }
                None
            }
            other => unreachable!("unresolved expression reached codegen: {other:?}"),
        }
    }

    fn never_check(&mut self, ty: &Type) {
        if ty.is_never() {
            self.term("unreachable");
        }
    }

    fn store_result(&mut self, res: &Option<String>, ty: &Type, v: Option<String>) {
        if let (Some(r), Some(v)) = (res, v) {
            if !self.terminated {
                self.emit(format!("store {} {v}, ptr {r}", lty(ty)));
            }
        }
    }

    fn load_result(&mut self, res: &Option<String>, ty: &Type) -> Option<String> {
        let r = res.as_ref()?;
        let t = self.tmp();
        self.emit(format!("{t} = load {}, ptr {r}", lty(ty)));
        Some(t)
    }

    fn args(&mut self, args: &[Expr]) -> Option<Vec<String>> {
        let mut out = Vec::with_capacity(args.len());
        for a in args {
            out.push(self.expr(a)?);
        }
        Some(out)
    }

    fn binop(&mut self, op: BinOp, ty: &Type, l: &str, r: &str, span: Span) -> String {
        use BinOp::*;
        let t = self.tmp();
        match ty {
            Type::Float => {
                let ins = match op {
                    Add => "fadd double",
                    Sub => "fsub double",
                    Mul => "fmul double",
                    Div => "fdiv double",
                    Rem => "frem double",
                    Eq => "fcmp oeq double",
                    Ne => "fcmp une double",
                    Lt => "fcmp olt double",
                    Le => "fcmp ole double",
                    Gt => "fcmp ogt double",
                    Ge => "fcmp oge double",
                    Pow => {
                        self.tmp -= 1;
                        return self.call("double", "@pow", &[("double", l), ("double", r)]);
                    }
                    _ => unreachable!(),
                };
                self.emit(format!("{t} = {ins} {l}, {r}"));
            }
            Type::Str => {
                self.tmp -= 1;
                return match op {
                    Add => self.call("ptr", "@ot_str_concat", &[("ptr", l), ("ptr", r)]),
                    Eq => self.call_bool("@ot_str_eq", &[("ptr", l), ("ptr", r)]),
                    Ne => {
                        let c = self.call("i64", "@ot_str_eq", &[("ptr", l), ("ptr", r)]);
                        let t = self.tmp();
                        self.emit(format!("{t} = icmp eq i64 {c}, 0"));
                        t
                    }
                    _ => {
                        let c = self.call("i64", "@ot_str_cmp", &[("ptr", l), ("ptr", r)]);
                        let pred = match op {
                            Lt => "slt",
                            Le => "sle",
                            Gt => "sgt",
                            _ => "sge",
                        };
                        let t = self.tmp();
                        self.emit(format!("{t} = icmp {pred} i64 {c}, 0"));
                        t
                    }
                };
            }
            Type::Bool => {
                let pred = if op == Eq { "eq" } else { "ne" };
                self.emit(format!("{t} = icmp {pred} i1 {l}, {r}"));
            }
            _ => {
                let loc_needed = matches!(op, Div | Rem | Pow);
                if loc_needed {
                    self.tmp -= 1;
                    let loc = self.loc(span);
                    let f = match op {
                        Div => "@ot_idiv",
                        Rem => "@ot_irem",
                        _ => "@ot_ipow",
                    };
                    return self.call("i64", f, &[("i64", l), ("i64", r), ("ptr", &loc)]);
                }
                if matches!(op, Shl | Shr) {
                    let m = self.tmp();
                    self.emit(format!("{m} = and i64 {r}, 63"));
                    let t2 = self.tmp();
                    let ins = if op == Shl { "shl" } else { "ashr" };
                    self.emit(format!("{t2} = {ins} i64 {l}, {m}"));
                    return t2;
                }
                let ins = match op {
                    Add => "add i64",
                    Sub => "sub i64",
                    Mul => "mul i64",
                    BitAnd => "and i64",
                    BitOr => "or i64",
                    BitXor => "xor i64",
                    Eq => "icmp eq i64",
                    Ne => "icmp ne i64",
                    Lt => "icmp slt i64",
                    Le => "icmp sle i64",
                    Gt => "icmp sgt i64",
                    Ge => "icmp sge i64",
                    _ => unreachable!(),
                };
                self.emit(format!("{t} = {ins} {l}, {r}"));
            }
        }
        t
    }

    #[allow(clippy::wrong_self_convention)]
    fn to_str(&mut self, v: &str, ty: &Type) -> String {
        match ty {
            Type::Str => v.to_string(),
            Type::Int => self.call("ptr", "@ot_int_to_str", &[("i64", v)]),
            Type::Float => self.call("ptr", "@ot_float_to_str", &[("double", v)]),
            Type::Bool => {
                let (tr, fa) = (self.str_const("true"), self.str_const("false"));
                let t = self.tmp();
                self.emit(format!("{t} = select i1 {v}, ptr {tr}, ptr {fa}"));
                t
            }
            Type::Enum(name) => {
                let n = self
                    .m
                    .enums
                    .iter()
                    .find(|e| &e.name == name)
                    .unwrap()
                    .variants
                    .len();
                let a = self.tmp();
                self.emit(format!(
                    "{a} = getelementptr [{n} x ptr], ptr @\"enum.{name}\", i64 0, i64 {v}"
                ));
                let t = self.tmp();
                self.emit(format!("{t} = load ptr, ptr {a}"));
                t
            }
            _ => unreachable!("not printable"),
        }
    }

    fn print_value(&mut self, v: &str, ty: &Type, flags: i64) {
        let f = flags.to_string();
        match ty {
            Type::Int => {
                self.call("void", "@ot_print_int", &[("i64", v), ("i64", &f)]);
            }
            Type::Float => {
                self.call("void", "@ot_print_float", &[("double", v), ("i64", &f)]);
            }
            _ => {
                let s = self.to_str(v, ty);
                self.call("void", "@ot_print_str", &[("ptr", &s), ("i64", &f)]);
            }
        }
    }

    fn builtin(&mut self, b: Builtin, args: &[Expr], e: &Expr) -> Option<String> {
        use Builtin::*;
        let span = e.span;
        match b {
            Print | Println | Eprintln => {
                let nl = if b == Print { 0 } else { 1 };
                let err = if b == Eprintln { 2 } else { 0 };
                match args.first() {
                    None => {
                        let s = self.str_const("");
                        let f = (nl | err).to_string();
                        self.call("void", "@ot_print_str", &[("ptr", &s), ("i64", &f)]);
                    }
                    Some(Expr {
                        kind: ExprKind::Interp(parts),
                        ..
                    }) => {
                        // Print the pieces directly instead of building a string.
                        let mut vals = Vec::new();
                        for p in parts {
                            match p {
                                InterpPart::Lit(s) => vals.push((self.str_const(s), Type::Str)),
                                InterpPart::Expr(x) => vals.push((self.expr(x)?, x.ty.clone())),
                            }
                        }
                        let n = vals.len();
                        for (i, (v, t)) in vals.iter().enumerate() {
                            let flags = if i + 1 == n { nl | err } else { err };
                            self.print_value(v, t, flags);
                        }
                    }
                    Some(a) => {
                        let v = self.expr(a)?;
                        self.print_value(&v, &a.ty, nl | err);
                    }
                }
                None
            }
            Assert => {
                let c = self.expr(&args[0])?;
                let msg = match args.get(1) {
                    Some(m) => self.expr(m)?,
                    None => "null".to_string(),
                };
                let ok = self.label("assert.ok");
                let fail = self.label("assert.fail");
                self.term(format!("br i1 {c}, label %{ok}, label %{fail}"));
                self.start_block(&fail);
                let loc = self.loc(span);
                self.call("void", "@ot_assert_fail", &[("ptr", &msg), ("ptr", &loc)]);
                self.term("unreachable");
                self.start_block(&ok);
                None
            }
            Exit | Panic => {
                let v = self.expr(&args[0])?;
                if b == Exit {
                    self.call("void", "@ot_exit", &[("i64", &v)]);
                } else {
                    self.call("void", "@ot_panic", &[("ptr", &v)]);
                }
                self.term("unreachable");
                None
            }
            ReadLine => Some(self.call("ptr", "@ot_read_line", &[])),
            Clock => Some(self.call("double", "@ot_clock", &[])),
            ParseInt | ParseFloat | Chr => {
                let v = self.expr(&args[0])?;
                let loc = self.loc(span);
                Some(match b {
                    ParseInt => self.call("i64", "@ot_parse_int", &[("ptr", &v), ("ptr", &loc)]),
                    ParseFloat => {
                        self.call("double", "@ot_parse_float", &[("ptr", &v), ("ptr", &loc)])
                    }
                    _ => self.call("ptr", "@ot_chr", &[("i64", &v), ("ptr", &loc)]),
                })
            }
            Sqrt | Sin | Cos | Tan | Exp | Ln | Floor | Ceil | Round => {
                let v = self.expr(&args[0])?;
                let f = match b {
                    Sqrt => "@sqrt",
                    Sin => "@sin",
                    Cos => "@cos",
                    Tan => "@tan",
                    Exp => "@exp",
                    Ln => "@log",
                    Floor => "@floor",
                    Ceil => "@ceil",
                    _ => "@round",
                };
                Some(self.call("double", f, &[("double", &v)]))
            }
            Atan2 => {
                let y = self.expr(&args[0])?;
                let x = self.expr(&args[1])?;
                Some(self.call("double", "@atan2", &[("double", &y), ("double", &x)]))
            }
            Abs => {
                let v = self.expr(&args[0])?;
                if e.ty == Type::Float {
                    return Some(self.call("double", "@fabs", &[("double", &v)]));
                }
                let neg = self.tmp();
                self.emit(format!("{neg} = sub i64 0, {v}"));
                let c = self.tmp();
                self.emit(format!("{c} = icmp slt i64 {v}, 0"));
                let t = self.tmp();
                self.emit(format!("{t} = select i1 {c}, i64 {neg}, i64 {v}"));
                Some(t)
            }
            Min | Max => {
                let a = self.expr(&args[0])?;
                let bv = self.expr(&args[1])?;
                let c = self.tmp();
                let ty = lty(&e.ty);
                let cmp = match (e.ty == Type::Float, b == Min) {
                    (true, true) => "fcmp olt",
                    (true, false) => "fcmp ogt",
                    (false, true) => "icmp slt",
                    (false, false) => "icmp sgt",
                };
                self.emit(format!("{c} = {cmp} {ty} {a}, {bv}"));
                let t = self.tmp();
                self.emit(format!("{t} = select i1 {c}, {ty} {a}, {ty} {bv}"));
                Some(t)
            }
            ArrLen => {
                let a = self.expr(&args[0])?;
                Some(self.call("i64", "@ot_arr_len", &[("ptr", &a)]))
            }
            ArrPush => {
                let a = self.expr(&args[0])?;
                let v = self.expr(&args[1])?;
                let slot = self.call("ptr", "@ot_arr_push_slot", &[("ptr", &a)]);
                self.emit(format!("store {} {v}, ptr {slot}", lty(&args[1].ty)));
                None
            }
            ArrPop => {
                let a = self.expr(&args[0])?;
                let loc = self.loc(span);
                let slot = self.call("ptr", "@ot_arr_pop_slot", &[("ptr", &a), ("ptr", &loc)]);
                let t = self.tmp();
                self.emit(format!("{t} = load {}, ptr {slot}", lty(&e.ty)));
                Some(t)
            }
            StrLen => {
                let s = self.expr(&args[0])?;
                Some(self.call("i64", "@ot_str_len", &[("ptr", &s)]))
            }
            StrByte => {
                let s = self.expr(&args[0])?;
                let i = self.expr(&args[1])?;
                let loc = self.loc(span);
                Some(self.call(
                    "i64",
                    "@ot_str_byte",
                    &[("ptr", &s), ("i64", &i), ("ptr", &loc)],
                ))
            }
            StrSlice => {
                let s = self.expr(&args[0])?;
                let a = self.expr(&args[1])?;
                let bb = self.expr(&args[2])?;
                let loc = self.loc(span);
                Some(self.call(
                    "ptr",
                    "@ot_str_slice",
                    &[("ptr", &s), ("i64", &a), ("i64", &bb), ("ptr", &loc)],
                ))
            }
            StrContains => {
                let s = self.expr(&args[0])?;
                let sub = self.expr(&args[1])?;
                Some(self.call_bool("@ot_str_contains", &[("ptr", &s), ("ptr", &sub)]))
            }
            StrFind => {
                let s = self.expr(&args[0])?;
                let sub = self.expr(&args[1])?;
                Some(self.call("i64", "@ot_str_find", &[("ptr", &s), ("ptr", &sub)]))
            }
        }
    }

    fn match_expr(&mut self, e: &Expr, scrut: &Expr, arms: &[MatchArm]) -> Option<String> {
        let res = if e.ty.is_value() {
            Some(self.alloca(lty(&e.ty)))
        } else {
            None
        };
        let s = self.expr(scrut)?;
        let end_l = self.label("match.end");
        let mut has_default = false;
        for arm in arms {
            let body_l = self.label("arm.body");
            let next_l = self.label("arm.next");
            let mut conds = Vec::new();
            let mut always = false;
            for p in &arm.pats {
                match p {
                    Pat::Wildcard(_) => always = true,
                    Pat::Int(n, _) => {
                        let t = self.tmp();
                        self.emit(format!("{t} = icmp eq i64 {s}, {n}"));
                        conds.push(t);
                    }
                    Pat::Variant(_, _, _, idx) => {
                        let t = self.tmp();
                        self.emit(format!("{t} = icmp eq i64 {s}, {idx}"));
                        conds.push(t);
                    }
                    Pat::Bool(b, _) => {
                        let t = self.tmp();
                        self.emit(format!("{t} = icmp eq i1 {s}, {b}"));
                        conds.push(t);
                    }
                    Pat::Str(lit, _) => {
                        let c = self.str_const(lit);
                        let t = self.call_bool("@ot_str_eq", &[("ptr", &s), ("ptr", &c)]);
                        conds.push(t);
                    }
                    Pat::Range(lo, hi, inc, _) => {
                        let a = self.tmp();
                        self.emit(format!("{a} = icmp sge i64 {s}, {lo}"));
                        let b = self.tmp();
                        let pred = if *inc { "sle" } else { "slt" };
                        self.emit(format!("{b} = icmp {pred} i64 {s}, {hi}"));
                        let t = self.tmp();
                        self.emit(format!("{t} = and i1 {a}, {b}"));
                        conds.push(t);
                    }
                }
            }
            if always {
                has_default = true;
                self.br(&body_l);
            } else {
                let mut c = conds[0].clone();
                for other in &conds[1..] {
                    let t = self.tmp();
                    self.emit(format!("{t} = or i1 {c}, {other}"));
                    c = t;
                }
                self.term(format!("br i1 {c}, label %{body_l}, label %{next_l}"));
            }
            self.start_block(&body_l);
            let v = self.expr(&arm.body);
            self.store_result(&res, &e.ty, v);
            self.br(&end_l);
            self.start_block(&next_l);
            if always {
                break;
            }
        }
        if !has_default {
            if e.ty.is_value() {
                self.term("unreachable");
            } else {
                self.br(&end_l);
            }
        } else {
            self.term("unreachable");
        }
        self.start_block(&end_l);
        self.load_result(&res, &e.ty)
    }

    fn for_loop(&mut self, local: LocalId, iter: &ForIter, body: &Block) {
        let var = self.locals[local as usize].clone();
        let var_ty = self.local_tys[local as usize].clone();
        match iter {
            ForIter::Range {
                start,
                end,
                inclusive,
            } => {
                let Some(a) = self.expr(start) else { return };
                let Some(b) = self.expr(end) else { return };
                self.emit(format!("store i64 {a}, ptr {var}"));
                let cond_l = self.label("for.cond");
                let body_l = self.label("for.body");
                let latch_l = self.label("for.next");
                let end_l = self.label("for.end");
                if *inclusive {
                    let c = self.tmp();
                    self.emit(format!("{c} = icmp sle i64 {a}, {b}"));
                    self.term(format!("br i1 {c}, label %{body_l}, label %{end_l}"));
                } else {
                    self.start_block(&cond_l);
                    let i = self.tmp();
                    self.emit(format!("{i} = load i64, ptr {var}"));
                    let c = self.tmp();
                    self.emit(format!("{c} = icmp slt i64 {i}, {b}"));
                    self.term(format!("br i1 {c}, label %{body_l}, label %{end_l}"));
                }
                self.start_block(&body_l);
                self.loops.push(LoopLabels {
                    brk: end_l.clone(),
                    cont: latch_l.clone(),
                    defer_depth: self.defers.len(),
                });
                self.block(body);
                self.loops.pop();
                self.start_block(&latch_l);
                let i = self.tmp();
                self.emit(format!("{i} = load i64, ptr {var}"));
                if *inclusive {
                    let c = self.tmp();
                    self.emit(format!("{c} = icmp eq i64 {i}, {b}"));
                    let inc_l = self.label("for.inc");
                    self.term(format!("br i1 {c}, label %{end_l}, label %{inc_l}"));
                    self.start_block(&inc_l);
                }
                let n = self.tmp();
                self.emit(format!("{n} = add i64 {i}, 1"));
                self.emit(format!("store i64 {n}, ptr {var}"));
                if *inclusive {
                    self.term(format!("br label %{body_l}"));
                } else {
                    self.term(format!("br label %{cond_l}"));
                }
                self.start_block(&end_l);
            }
            ForIter::Each(arr_e) => {
                let Some(arr) = self.expr(arr_e) else { return };
                let idx = self.alloca("i64");
                self.emit(format!("store i64 0, ptr {idx}"));
                let cond_l = self.label("each.cond");
                let body_l = self.label("each.body");
                let latch_l = self.label("each.next");
                let end_l = self.label("each.end");
                self.start_block(&cond_l);
                let i = self.tmp();
                self.emit(format!("{i} = load i64, ptr {idx}"));
                let n = self.call("i64", "@ot_arr_len", &[("ptr", &arr)]);
                let c = self.tmp();
                self.emit(format!("{c} = icmp slt i64 {i}, {n}"));
                self.term(format!("br i1 {c}, label %{body_l}, label %{end_l}"));
                self.start_block(&body_l);
                let data = self.call("ptr", "@ot_arr_data", &[("ptr", &arr)]);
                let slot = self.tmp();
                self.emit(format!("{slot} = getelementptr i64, ptr {data}, i64 {i}"));
                let v = self.tmp();
                self.emit(format!("{v} = load {}, ptr {slot}", lty(&var_ty)));
                self.emit(format!("store {} {v}, ptr {var}", lty(&var_ty)));
                self.loops.push(LoopLabels {
                    brk: end_l.clone(),
                    cont: latch_l.clone(),
                    defer_depth: self.defers.len(),
                });
                self.block(body);
                self.loops.pop();
                self.start_block(&latch_l);
                let i2 = self.tmp();
                self.emit(format!("{i2} = load i64, ptr {idx}"));
                let n2 = self.tmp();
                self.emit(format!("{n2} = add i64 {i2}, 1"));
                self.emit(format!("store i64 {n2}, ptr {idx}"));
                self.term(format!("br label %{cond_l}"));
                self.start_block(&end_l);
            }
        }
    }
}

fn extern_param(t: &Type) -> String {
    if *t == Type::Bool {
        "i1 zeroext".to_string()
    } else {
        lty(t).to_string()
    }
}
