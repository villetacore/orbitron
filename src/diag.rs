//! Source positions and compiler diagnostics.

use std::fmt;

/// Position of a token or node in a source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub file: u32,
    pub line: u32,
    pub col: u32,
}

impl Span {
    pub fn new(file: u32, line: u32, col: u32) -> Self {
        Span { file, line, col }
    }
}

/// A single compiler error.
#[derive(Debug, Clone)]
pub struct Diag {
    pub span: Span,
    pub msg: String,
    pub note: Option<String>,
}

impl Diag {
    pub fn new(span: Span, msg: impl Into<String>) -> Self {
        Diag {
            span,
            msg: msg.into(),
            note: None,
        }
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// All source files loaded during one compilation. Used to render diagnostics
/// with the offending line and a caret.
#[derive(Default)]
pub struct SourceMap {
    files: Vec<(String, String)>,
}

impl SourceMap {
    pub fn add(&mut self, path: String, text: String) -> u32 {
        self.files.push((path, text));
        (self.files.len() - 1) as u32
    }

    pub fn path(&self, file: u32) -> &str {
        self.files
            .get(file as usize)
            .map(|f| f.0.as_str())
            .unwrap_or("<неизвестно>")
    }

    pub fn render(&self, d: &Diag) -> String {
        let mut out = String::new();
        let path = self.path(d.span.file);
        out.push_str(&format!("ошибка: {}\n", d.msg));
        out.push_str(&format!("  --> {}:{}:{}\n", path, d.span.line, d.span.col));
        if let Some((_, text)) = self.files.get(d.span.file as usize) {
            if let Some(line) = text.lines().nth(d.span.line.saturating_sub(1) as usize) {
                let num = d.span.line.to_string();
                let pad = " ".repeat(num.len());
                let caret_pad: String = line
                    .chars()
                    .take(d.span.col.saturating_sub(1) as usize)
                    .map(|c| if c == '\t' { '\t' } else { ' ' })
                    .collect();
                out.push_str(&format!("{pad} |\n{num} | {line}\n{pad} | {caret_pad}^\n"));
            }
        }
        if let Some(note) = &d.note {
            out.push_str(&format!("  = примечание: {note}\n"));
        }
        out
    }

    pub fn render_all(&self, diags: &[Diag]) -> String {
        let mut out = String::new();
        for d in diags {
            out.push_str(&self.render(d));
            out.push('\n');
        }
        let n = diags.len();
        out.push_str(&format!("компиляция прервана: {} {}", n, plural_errors(n)));
        out
    }
}

fn plural_errors(n: usize) -> &'static str {
    let (m10, m100) = (n % 10, n % 100);
    if m10 == 1 && m100 != 11 {
        "ошибка"
    } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
        "ошибки"
    } else {
        "ошибок"
    }
}

impl fmt::Display for Diag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.span.line, self.span.col, self.msg)
    }
}
