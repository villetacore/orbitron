//! `orbitron fmt` — canonical source formatter.
//!
//! Re-prints the parsed AST in one style (4-space indents, `{` on the same
//! line, spaces around binary operators), keeping comments, blank lines
//! between statements and the original spelling of numeric literals.

use crate::ast::*;
use crate::diag::{Diag, Span};
use crate::lexer::{tokenize, Comment};
use crate::parser::parse_program;

pub fn format_source(src: &str) -> Result<String, Diag> {
    let lexed = tokenize(src, 0)?;
    let items = parse_program(lexed.tokens)?;
    let mut f = Fmt {
        out: String::new(),
        comments: lexed.comments,
        ci: 0,
        lines: src.lines().collect(),
        indent: 0,
        trailing: None,
        at_line_start: true,
    };
    f.program(&items);
    Ok(f.out)
}

struct Fmt<'a> {
    out: String,
    comments: Vec<Comment>,
    ci: usize,
    lines: Vec<&'a str>,
    indent: usize,
    trailing: Option<String>,
    at_line_start: bool,
}

impl<'a> Fmt<'a> {
    // ── Low-level writing ────────────────────────────────────────────────────

    fn w(&mut self, s: &str) {
        if self.at_line_start {
            for _ in 0..self.indent {
                self.out.push_str("    ");
            }
            self.at_line_start = false;
        }
        self.out.push_str(s);
    }

    fn nl(&mut self) {
        if let Some(c) = self.trailing.take() {
            self.out.push(' ');
            self.out.push_str(&c);
        }
        self.out.push('\n');
        self.at_line_start = true;
    }

    fn blank(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with("\n\n") && !self.out.ends_with("{\n") {
            self.out.push('\n');
        }
    }

    /// Emits own-line comments that appear before `line`.
    fn comments_before(&mut self, line: u32) {
        while self.ci < self.comments.len() && self.comments[self.ci].line < line {
            let c = self.comments[self.ci].clone();
            self.ci += 1;
            if self.has_blank_before(c.line) {
                self.blank();
            }
            for (i, l) in c.text.lines().enumerate() {
                if i > 0 {
                    self.nl();
                }
                self.w(if i == 0 { l } else { l.trim_end() });
            }
            self.nl();
        }
    }

    /// Keeps a blank line that separates preceding comments from a node.
    fn gap_before(&mut self, line: u32) {
        if line >= 2
            && self
                .lines
                .get(line as usize - 2)
                .is_some_and(|l| l.trim().is_empty())
        {
            self.blank();
        }
    }

    /// Picks up a comment written after code on `line`.
    fn take_trailing(&mut self, line: u32) {
        if self.ci < self.comments.len()
            && self.comments[self.ci].line == line
            && !self.comments[self.ci].own_line
        {
            let text = self.comments[self.ci].text.clone();
            self.ci += 1;
            self.trailing = Some(match self.trailing.take() {
                Some(prev) => format!("{prev} {text}"),
                None => text,
            });
        }
    }

    fn has_blank_before(&self, line: u32) -> bool {
        let mut l = line as usize;
        while l >= 2 {
            let prev = self.lines.get(l - 2).map(|s| s.trim()).unwrap_or("");
            if prev.is_empty() {
                return true;
            }
            if prev.starts_with("//") || prev.starts_with("/*") || prev.starts_with('*') {
                l -= 1;
                continue;
            }
            return false;
        }
        false
    }

    /// Starts a node (item, statement, field, arm) located at `span`.
    fn begin(&mut self, span: Span, first: bool) {
        let had_blank = self.has_blank_before(span.line);
        if !first && had_blank && self.ci_comment_line_before(span.line).is_none() {
            self.blank();
        }
        self.comments_before(span.line);
        if !first {
            self.gap_before(span.line);
        }
        self.take_trailing(span.line);
    }

    fn ci_comment_line_before(&self, line: u32) -> Option<u32> {
        let c = self.comments.get(self.ci)?;
        if c.line < line {
            Some(c.line)
        } else {
            None
        }
    }

    fn source_text_at(&self, span: Span) -> Option<String> {
        let line = self.lines.get(span.line.checked_sub(1)? as usize)?;
        let rest: String = line
            .chars()
            .skip(span.col.saturating_sub(1) as usize)
            .collect();
        let chars: Vec<char> = rest.chars().collect();
        if chars.first() == Some(&'\'') {
            let mut s = String::from("'");
            let mut i = 1;
            while i < chars.len() {
                s.push(chars[i]);
                if chars[i] == '\\' && i + 1 < chars.len() {
                    s.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                if chars[i] == '\'' {
                    return Some(s);
                }
                i += 1;
            }
            return None;
        }
        let mut s = String::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let ok = c.is_ascii_alphanumeric()
                || c == '_'
                || (c == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit()))
                || ((c == '+' || c == '-')
                    && matches!(s.chars().last(), Some('e' | 'E'))
                    && !s.starts_with("0x"));
            if !ok {
                break;
            }
            s.push(c);
            i += 1;
        }
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }

    // ── Items ────────────────────────────────────────────────────────────────

    fn program(&mut self, items: &[Item]) {
        let mut prev: Option<&Item> = None;
        for item in items {
            let compact = |k: &ItemKind| {
                matches!(
                    k,
                    ItemKind::Import(_) | ItemKind::Const(_) | ItemKind::Extern(_)
                )
            };
            let same_group = prev.is_some_and(|p| {
                compact(&p.kind)
                    && std::mem::discriminant(&p.kind) == std::mem::discriminant(&item.kind)
            });
            if prev.is_some() && (!same_group || self.has_blank_before(item.span.line)) {
                self.blank();
            }
            self.comments_before(item.span.line);
            self.gap_before(item.span.line);
            self.take_trailing(item.span.line);
            self.item(item);
            prev = Some(item);
        }
        self.comments_before(u32::MAX);
        while self.out.ends_with("\n\n") {
            self.out.pop();
        }
    }

    fn item(&mut self, item: &Item) {
        match &item.kind {
            ItemKind::Import(p) => {
                self.w(&format!("import {};", quote(p)));
                self.nl();
            }
            ItemKind::Const(c) => {
                self.w(&format!("const {}", c.name));
                if let Some(t) = &c.ty {
                    self.w(&format!(": {}", type_str(t)));
                }
                self.w(" = ");
                self.expr(&c.value);
                self.w(";");
                self.nl();
            }
            ItemKind::Extern(e) => {
                self.w(&format!(
                    "extern fn {}({})",
                    e.name,
                    params_str(false, &e.params)
                ));
                if let Some(r) = &e.ret {
                    self.w(&format!(" -> {}", type_str(r)));
                }
                self.w(";");
                self.nl();
            }
            ItemKind::Fn(f) => self.fn_decl(f),
            ItemKind::Struct(s) => {
                if s.fields.iter().all(|f| f.span.line == item.span.line) && !s.fields.is_empty() {
                    let fs: Vec<String> = s
                        .fields
                        .iter()
                        .map(|f| format!("{}: {}", f.name, type_str(&f.ty)))
                        .collect();
                    self.w(&format!("struct {} {{ {} }}", s.name, fs.join(", ")));
                    self.nl();
                    return;
                }
                if s.fields.is_empty() {
                    self.w(&format!("struct {} {{}}", s.name));
                    self.nl();
                    return;
                }
                self.w(&format!("struct {} {{", s.name));
                self.nl();
                self.indent += 1;
                for (i, f) in s.fields.iter().enumerate() {
                    self.begin(f.span, i == 0);
                    self.w(&format!("{}: {},", f.name, type_str(&f.ty)));
                    self.nl();
                }
                self.comments_before(self.closing_line(item.span));
                self.indent -= 1;
                self.w("}");
                self.nl();
            }
            ItemKind::Enum(e) => {
                let one_line = e.variants.iter().all(|(_, s)| s.line == item.span.line);
                if one_line {
                    let vs: Vec<&str> = e.variants.iter().map(|(v, _)| v.as_str()).collect();
                    self.w(&format!("enum {} {{ {} }}", e.name, vs.join(", ")));
                    self.nl();
                } else {
                    self.w(&format!("enum {} {{", e.name));
                    self.nl();
                    self.indent += 1;
                    for (i, (v, s)) in e.variants.iter().enumerate() {
                        self.begin(*s, i == 0);
                        self.w(&format!("{v},"));
                        self.nl();
                    }
                    self.comments_before(self.closing_line(item.span));
                    self.indent -= 1;
                    self.w("}");
                    self.nl();
                }
            }
            ItemKind::Impl(imp) => {
                self.w(&format!("impl {} {{", imp.type_name));
                self.nl();
                self.indent += 1;
                for (i, m) in imp.methods.iter().enumerate() {
                    if i > 0 {
                        self.blank();
                    }
                    self.comments_before(m.span.line);
                    self.take_trailing(m.span.line);
                    self.fn_decl(m);
                }
                self.comments_before(self.closing_line(item.span));
                self.indent -= 1;
                self.w("}");
                self.nl();
            }
        }
    }

    /// Finds the line of the `}` that closes a top-level item (first line at
    /// column 0 starting with `}` after the item start).
    fn closing_line(&self, start: Span) -> u32 {
        for (i, l) in self.lines.iter().enumerate().skip(start.line as usize - 1) {
            if l.starts_with('}') || (i + 1 == start.line as usize && l.trim_end().ends_with('}')) {
                return i as u32 + 1;
            }
        }
        u32::MAX
    }

    fn fn_decl(&mut self, f: &FnDecl) {
        self.w(&format!(
            "fn {}({})",
            f.name,
            params_str(f.has_self, &f.params)
        ));
        if let Some(r) = &f.ret {
            self.w(&format!(" -> {}", type_str(r)));
        }
        self.w(" ");
        self.block(&f.body, true);
        self.nl();
    }

    // ── Blocks and statements ────────────────────────────────────────────────

    fn block_is_inline(&self, b: &Block) -> bool {
        let n = b.stmts.len() + usize::from(b.tail.is_some());
        let no_comments = !self
            .comments
            .iter()
            .skip(self.ci)
            .any(|c| c.line >= b.span.line && c.line <= b.end_line);
        b.span.line == b.end_line && n <= 1 && no_comments && !self.contains_multiline(b)
    }

    fn contains_multiline(&self, b: &Block) -> bool {
        b.stmts.iter().any(|s| matches!(&s.kind, StmtKind::Expr { expr, .. } if matches!(expr.kind, ExprKind::Match { .. })))
            || b.tail.as_ref().is_some_and(|t| matches!(t.kind, ExprKind::Match { .. }))
    }

    /// Prints `{ ... }`. Leaves the cursor right after `}`.
    fn block(&mut self, b: &Block, allow_inline: bool) {
        if b.stmts.is_empty() && b.tail.is_none() && b.span.line == b.end_line {
            self.w("{}");
            return;
        }
        if allow_inline && self.block_is_inline(b) {
            self.w("{ ");
            if let Some(s) = b.stmts.first() {
                self.stmt_inline(s);
            }
            if let Some(t) = &b.tail {
                self.expr(t);
            }
            self.w(" }");
            return;
        }
        self.w("{");
        self.nl();
        self.indent += 1;
        let mut first = true;
        for s in &b.stmts {
            self.begin(s.span, first);
            self.stmt_inline(s);
            self.nl();
            first = false;
        }
        if let Some(t) = &b.tail {
            self.begin(t.span, first);
            self.expr(t);
            self.nl();
        }
        self.comments_before(b.end_line);
        self.indent -= 1;
        self.w("}");
    }

    fn stmt_inline(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Let {
                name,
                mutable,
                ty,
                init,
                ..
            } => {
                self.w(if *mutable { "var " } else { "let " });
                self.w(name);
                if let Some(t) = ty {
                    self.w(&format!(": {}", type_str(t)));
                }
                self.w(" = ");
                self.expr(init);
                self.w(";");
            }
            StmtKind::Assign { target, op, value } => {
                self.expr(target);
                match op {
                    Some(op) => self.w(&format!(" {}= ", op.symbol())),
                    None => self.w(" = "),
                }
                self.expr(value);
                self.w(";");
            }
            StmtKind::Expr { expr, semi } => {
                self.expr(expr);
                if *semi && !expr.is_block_like() {
                    self.w(";");
                } else if *semi {
                    // `if ... {};` — the semicolon is redundant
                }
            }
            StmtKind::Defer(e) => {
                self.w("defer ");
                self.expr(e);
                if !e.is_block_like() {
                    self.w(";");
                }
            }
        }
    }

    // ── Expressions ──────────────────────────────────────────────────────────

    fn exprs(&mut self, es: &[Expr]) {
        for (i, e) in es.iter().enumerate() {
            if i > 0 {
                self.w(", ");
            }
            self.expr(e);
        }
    }

    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Int(n) => {
                let s = self.source_text_at(e.span).unwrap_or_else(|| n.to_string());
                self.w(&s);
            }
            ExprKind::Float(f) => {
                let s = self
                    .source_text_at(e.span)
                    .unwrap_or_else(|| format!("{f:?}"));
                self.w(&s);
            }
            ExprKind::Bool(b) => self.w(if *b { "true" } else { "false" }),
            ExprKind::Str(s) => self.w(&quote(s)),
            ExprKind::Interp(parts) => {
                self.w("\"");
                for p in parts {
                    match p {
                        InterpPart::Lit(s) => self.w(&escape(s)),
                        InterpPart::Expr(x) => {
                            self.w("{");
                            self.expr(x);
                            self.w("}");
                        }
                    }
                }
                self.w("\"");
            }
            ExprKind::Ident(n) => self.w(n),
            ExprKind::SelfRef => self.w("self"),
            ExprKind::Path(a, b) => self.w(&format!("{a}::{b}")),
            ExprKind::Paren(x) => {
                self.w("(");
                self.expr(x);
                self.w(")");
            }
            ExprKind::Unary(op, x) => {
                self.w(match op {
                    UnOp::Neg => "-",
                    UnOp::Not => "!",
                    UnOp::BitNot => "~",
                });
                self.expr(x);
            }
            ExprKind::Binary(op, l, r) => {
                self.expr(l);
                self.w(&format!(" {} ", op.symbol()));
                self.expr(r);
            }
            ExprKind::Pipe(l, r) => {
                self.expr(l);
                self.w(" |> ");
                self.expr(r);
            }
            ExprKind::Cast(x, t) => {
                self.expr(x);
                self.w(&format!(" as {}", type_str(t)));
            }
            ExprKind::Call { callee, args } => {
                self.expr(callee);
                self.w("(");
                self.exprs(args);
                self.w(")");
            }
            ExprKind::MethodCall { recv, method, args } => {
                self.expr(recv);
                self.w(&format!(".{method}("));
                self.exprs(args);
                self.w(")");
            }
            ExprKind::Field { obj, name } => {
                self.expr(obj);
                self.w(&format!(".{name}"));
            }
            ExprKind::Index { obj, idx } => {
                self.expr(obj);
                self.w("[");
                self.expr(idx);
                self.w("]");
            }
            ExprKind::StructLit { name, fields } => {
                if fields.is_empty() {
                    self.w(&format!("{name} {{}}"));
                    return;
                }
                self.w(&format!("{name} {{ "));
                for (i, (f, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        self.w(", ");
                    }
                    if matches!(&v.kind, ExprKind::Ident(n) if n == f) {
                        self.w(f);
                    } else {
                        self.w(&format!("{f}: "));
                        self.expr(v);
                    }
                }
                self.w(" }");
            }
            ExprKind::ArrayLit(items) => {
                if items.iter().any(|i| i.span.line != e.span.line) {
                    self.w("[");
                    self.nl();
                    self.indent += 1;
                    for (i, it) in items.iter().enumerate() {
                        self.begin(it.span, i == 0);
                        self.expr(it);
                        self.w(",");
                        self.nl();
                    }
                    self.indent -= 1;
                    self.w("]");
                } else {
                    self.w("[");
                    self.exprs(items);
                    self.w("]");
                }
            }
            ExprKind::ArrayRepeat { value, count } => {
                self.w("[");
                self.expr(value);
                self.w("; ");
                self.expr(count);
                self.w("]");
            }
            ExprKind::Lambda { params, ret, body } => {
                self.w(&format!("fn({})", params_str(false, params)));
                if let Some(r) = ret {
                    self.w(&format!(" -> {}", type_str(r)));
                }
                self.w(" ");
                self.block(body, true);
            }
            ExprKind::If { cond, then, els } => {
                self.w("if ");
                self.expr(cond);
                self.w(" ");
                self.block(then, true);
                if let Some(els) = els {
                    self.w(" else ");
                    match &els.kind {
                        ExprKind::Block(b) => self.block(b, true),
                        _ => self.expr(els),
                    }
                }
            }
            ExprKind::Match { scrut, arms } => {
                self.w("match ");
                self.expr(scrut);
                self.w(" {");
                self.nl();
                self.indent += 1;
                for (i, arm) in arms.iter().enumerate() {
                    self.begin(arm.span, i == 0);
                    for (j, p) in arm.pats.iter().enumerate() {
                        if j > 0 {
                            self.w(" | ");
                        }
                        self.w(&pat_str(p));
                    }
                    self.w(" => ");
                    match &arm.body.kind {
                        ExprKind::Block(b) => self.block(b, true),
                        _ => {
                            self.expr(&arm.body);
                            self.w(",");
                        }
                    }
                    self.nl();
                }
                self.indent -= 1;
                self.w("}");
            }
            ExprKind::Block(b) => self.block(b, true),
            ExprKind::While { cond, body } => {
                self.w("while ");
                self.expr(cond);
                self.w(" ");
                self.block(body, true);
            }
            ExprKind::For {
                var, iter, body, ..
            } => {
                self.w(&format!("for {var} in "));
                match iter {
                    ForIter::Range {
                        start,
                        end,
                        inclusive,
                    } => {
                        self.expr(start);
                        self.w(if *inclusive { "..=" } else { ".." });
                        self.expr(end);
                    }
                    ForIter::Each(x) => self.expr(x),
                }
                self.w(" ");
                self.block(body, true);
            }
            ExprKind::Loop(b) => {
                self.w("loop ");
                self.block(b, true);
            }
            ExprKind::Break => self.w("break"),
            ExprKind::Continue => self.w("continue"),
            ExprKind::Return(v) => {
                self.w("return");
                if let Some(v) = v {
                    self.w(" ");
                    self.expr(v);
                }
            }
            _ => self.w("/* ? */"),
        }
    }
}

fn escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn quote(s: &str) -> String {
    format!("\"{}\"", escape(s))
}

fn type_str(t: &TypeExpr) -> String {
    match t {
        TypeExpr::Named(n, _) => n.clone(),
        TypeExpr::Array(inner, _) => format!("[{}]", type_str(inner)),
        TypeExpr::Fn(ps, r, _) => {
            let ps: Vec<String> = ps.iter().map(type_str).collect();
            match r {
                Some(r) => format!("fn({}) -> {}", ps.join(", "), type_str(r)),
                None => format!("fn({})", ps.join(", ")),
            }
        }
    }
}

fn params_str(has_self: bool, params: &[Param]) -> String {
    let mut parts = Vec::new();
    if has_self {
        parts.push("self".to_string());
    }
    for p in params {
        parts.push(format!("{}: {}", p.name, type_str(&p.ty)));
    }
    parts.join(", ")
}

fn pat_str(p: &Pat) -> String {
    match p {
        Pat::Wildcard(_) => "_".into(),
        Pat::Int(n, _) => n.to_string(),
        Pat::Str(s, _) => quote(s),
        Pat::Bool(b, _) => b.to_string(),
        Pat::Range(a, b, inc, _) => format!("{a}{}{b}", if *inc { "..=" } else { ".." }),
        Pat::Variant(e, v, _, _) => format!("{e}::{v}"),
    }
}

#[cfg(test)]
mod tests {
    use super::format_source;

    #[test]
    fn idempotent_and_keeps_comments() {
        let src = "// header\nfn main() {\n    let x = 0xff; // hex\n\n    // note\n    if x > 1 { println(\"big {x}\"); }\n}\n";
        let once = format_source(src).unwrap();
        assert!(once.contains("0xff"));
        assert!(once.contains("// hex"));
        assert!(once.contains("// note"));
        assert!(once.contains("// header"));
        let twice = format_source(&once).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn normalizes_spacing() {
        let src = "fn  add( a:int,b:int )->int{a+b}";
        assert_eq!(
            format_source(src).unwrap(),
            "fn add(a: int, b: int) -> int { a + b }\n"
        );
    }
}
