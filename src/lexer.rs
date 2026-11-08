//! Lexer: source text → tokens (+ comments, kept separately for `orbitron fmt`).

use crate::diag::{Diag, Span};

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Int(i64),
    Float(f64),
    /// String literal. Interpolated holes `{expr}` are pre-lexed into tokens.
    Str(Vec<StrPart>),
    Ident(String),

    // Keywords
    Let,
    Var,
    Const,
    Fn,
    Return,
    If,
    Else,
    While,
    For,
    In,
    Loop,
    Break,
    Continue,
    Match,
    Struct,
    Impl,
    Enum,
    Import,
    Extern,
    Defer,
    True,
    False,
    SelfKw,
    As,

    // Delimiters
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semi,
    Colon,
    ColonColon,
    Dot,
    DotDot,
    DotDotEq,
    Arrow,
    FatArrow,

    // Operators
    Plus,
    Minus,
    Star,
    StarStar,
    Slash,
    Percent,
    Amp,
    Pipe,
    Caret,
    Tilde,
    Bang,
    Shl,
    Shr,
    AndAnd,
    OrOr,
    PipeGt,
    Assign,
    EqEq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    AmpEq,
    PipeEq,
    CaretEq,
    ShlEq,
    ShrEq,

    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StrPart {
    Lit(String),
    /// Tokens of an interpolated expression (terminated by `Eof`).
    Expr(Vec<Token>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

/// A source comment. `own_line` is true when no code precedes it on its line.
#[derive(Debug, Clone)]
pub struct Comment {
    pub line: u32,
    pub text: String,
    pub own_line: bool,
}

pub fn keyword(s: &str) -> Option<Tok> {
    Some(match s {
        "let" => Tok::Let,
        "var" => Tok::Var,
        "const" => Tok::Const,
        "fn" => Tok::Fn,
        "return" => Tok::Return,
        "if" => Tok::If,
        "else" => Tok::Else,
        "while" => Tok::While,
        "for" => Tok::For,
        "in" => Tok::In,
        "loop" => Tok::Loop,
        "break" => Tok::Break,
        "continue" => Tok::Continue,
        "match" => Tok::Match,
        "struct" => Tok::Struct,
        "impl" => Tok::Impl,
        "enum" => Tok::Enum,
        "import" => Tok::Import,
        "extern" => Tok::Extern,
        "defer" => Tok::Defer,
        "true" => Tok::True,
        "false" => Tok::False,
        "self" => Tok::SelfKw,
        "as" => Tok::As,
        _ => return None,
    })
}

pub struct Lexer {
    src: Vec<char>,
    pos: usize,
    line: u32,
    col: u32,
    file: u32,
    last_code_line: u32,
    pub comments: Vec<Comment>,
}

pub struct Lexed {
    pub tokens: Vec<Token>,
    pub comments: Vec<Comment>,
}

pub fn tokenize(src: &str, file: u32) -> Result<Lexed, Diag> {
    let mut lx = Lexer {
        src: src.chars().collect(),
        pos: 0,
        line: 1,
        col: 1,
        file,
        last_code_line: 0,
        comments: Vec::new(),
    };
    let mut tokens = Vec::new();
    loop {
        let t = lx.next_token()?;
        let eof = t.tok == Tok::Eof;
        tokens.push(t);
        if eof {
            break;
        }
    }
    Ok(Lexed {
        tokens,
        comments: lx.comments,
    })
}

impl Lexer {
    fn peek(&self) -> Option<char> {
        self.src.get(self.pos).copied()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.src.get(self.pos + n).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.src.get(self.pos).copied()?;
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn span(&self) -> Span {
        Span::new(self.file, self.line, self.col)
    }

    fn err(&self, span: Span, msg: impl Into<String>) -> Diag {
        Diag::new(span, msg)
    }

    fn skip_trivia(&mut self) -> Result<(), Diag> {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.bump();
                }
                Some('/') if self.peek_at(1) == Some('/') => {
                    let line = self.line;
                    let own_line = self.last_code_line != line;
                    let mut text = String::new();
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        text.push(c);
                        self.bump();
                    }
                    self.comments.push(Comment {
                        line,
                        text: text.trim_end().to_string(),
                        own_line,
                    });
                }
                Some('/') if self.peek_at(1) == Some('*') => {
                    let start = self.span();
                    let own_line = self.last_code_line != self.line;
                    let mut text = String::new();
                    let mut depth = 0;
                    loop {
                        match (self.peek(), self.peek_at(1)) {
                            (Some('/'), Some('*')) => {
                                depth += 1;
                                text.push_str("/*");
                                self.bump();
                                self.bump();
                            }
                            (Some('*'), Some('/')) => {
                                depth -= 1;
                                text.push_str("*/");
                                self.bump();
                                self.bump();
                                if depth == 0 {
                                    break;
                                }
                            }
                            (Some(c), _) => {
                                text.push(c);
                                self.bump();
                            }
                            (None, _) => {
                                return Err(self.err(start, "незакрытый блочный комментарий"))
                            }
                        }
                    }
                    self.comments.push(Comment {
                        line: start.line,
                        text,
                        own_line,
                    });
                }
                _ => return Ok(()),
            }
        }
    }

    fn next_token(&mut self) -> Result<Token, Diag> {
        self.skip_trivia()?;
        let span = self.span();
        let c = match self.peek() {
            None => {
                return Ok(Token {
                    tok: Tok::Eof,
                    span,
                })
            }
            Some(c) => c,
        };
        self.last_code_line = self.line;

        if c.is_ascii_digit() {
            let tok = self.number(span)?;
            return Ok(Token { tok, span });
        }
        if c.is_alphabetic() || c == '_' {
            let mut s = String::new();
            while let Some(c) = self.peek() {
                if c.is_alphanumeric() || c == '_' {
                    s.push(c);
                    self.bump();
                } else {
                    break;
                }
            }
            let tok = keyword(&s).unwrap_or(Tok::Ident(s));
            return Ok(Token { tok, span });
        }
        if c == '"' {
            self.bump();
            let parts = self.string(span)?;
            return Ok(Token {
                tok: Tok::Str(parts),
                span,
            });
        }
        if c == '\'' {
            self.bump();
            let ch = match self.bump() {
                Some('\\') => self.escape(span)?,
                Some('\'') | None => return Err(self.err(span, "пустой символьный литерал")),
                Some(c) => c,
            };
            if self.bump() != Some('\'') {
                return Err(self.err(
                    span,
                    "символьный литерал должен содержать ровно один символ",
                ));
            }
            return Ok(Token {
                tok: Tok::Int(ch as i64),
                span,
            });
        }

        self.bump();
        let next = self.peek();
        let two = |lx: &mut Self, t: Tok| {
            lx.bump();
            t
        };
        let tok = match (c, next) {
            ('(', _) => Tok::LParen,
            (')', _) => Tok::RParen,
            ('{', _) => Tok::LBrace,
            ('}', _) => Tok::RBrace,
            ('[', _) => Tok::LBracket,
            (']', _) => Tok::RBracket,
            (',', _) => Tok::Comma,
            (';', _) => Tok::Semi,
            ('~', _) => Tok::Tilde,
            (':', Some(':')) => two(self, Tok::ColonColon),
            (':', _) => Tok::Colon,
            ('.', Some('.')) => {
                self.bump();
                if self.peek() == Some('=') {
                    self.bump();
                    Tok::DotDotEq
                } else {
                    Tok::DotDot
                }
            }
            ('.', _) => Tok::Dot,
            ('-', Some('>')) => two(self, Tok::Arrow),
            ('-', Some('=')) => two(self, Tok::MinusEq),
            ('-', _) => Tok::Minus,
            ('+', Some('=')) => two(self, Tok::PlusEq),
            ('+', _) => Tok::Plus,
            ('*', Some('*')) => two(self, Tok::StarStar),
            ('*', Some('=')) => two(self, Tok::StarEq),
            ('*', _) => Tok::Star,
            ('/', Some('=')) => two(self, Tok::SlashEq),
            ('/', _) => Tok::Slash,
            ('%', Some('=')) => two(self, Tok::PercentEq),
            ('%', _) => Tok::Percent,
            ('^', Some('=')) => two(self, Tok::CaretEq),
            ('^', _) => Tok::Caret,
            ('&', Some('&')) => two(self, Tok::AndAnd),
            ('&', Some('=')) => two(self, Tok::AmpEq),
            ('&', _) => Tok::Amp,
            ('|', Some('|')) => two(self, Tok::OrOr),
            ('|', Some('>')) => two(self, Tok::PipeGt),
            ('|', Some('=')) => two(self, Tok::PipeEq),
            ('|', _) => Tok::Pipe,
            ('!', Some('=')) => two(self, Tok::Ne),
            ('!', _) => Tok::Bang,
            ('=', Some('=')) => two(self, Tok::EqEq),
            ('=', Some('>')) => two(self, Tok::FatArrow),
            ('=', _) => Tok::Assign,
            ('<', Some('<')) => {
                self.bump();
                if self.peek() == Some('=') {
                    self.bump();
                    Tok::ShlEq
                } else {
                    Tok::Shl
                }
            }
            ('<', Some('=')) => two(self, Tok::Le),
            ('<', _) => Tok::Lt,
            ('>', Some('>')) => {
                self.bump();
                if self.peek() == Some('=') {
                    self.bump();
                    Tok::ShrEq
                } else {
                    Tok::Shr
                }
            }
            ('>', Some('=')) => two(self, Tok::Ge),
            ('>', _) => Tok::Gt,
            _ => return Err(self.err(span, format!("неожиданный символ '{c}'"))),
        };
        Ok(Token { tok, span })
    }

    fn number(&mut self, span: Span) -> Result<Tok, Diag> {
        let radix = match (self.peek(), self.peek_at(1)) {
            (Some('0'), Some('x')) => 16,
            (Some('0'), Some('b')) => 2,
            (Some('0'), Some('o')) => 8,
            _ => 10,
        };
        if radix != 10 {
            self.bump();
            self.bump();
            let mut digits = String::new();
            while let Some(c) = self.peek() {
                if c == '_' {
                    self.bump();
                } else if c.is_digit(radix) {
                    digits.push(c);
                    self.bump();
                } else {
                    break;
                }
            }
            if digits.is_empty() {
                return Err(self.err(span, "в числовом литерале нет цифр"));
            }
            return u64::from_str_radix(&digits, radix)
                .map(|v| Tok::Int(v as i64))
                .map_err(|_| self.err(span, "числовой литерал не помещается в 64 бита"));
        }

        let mut text = String::new();
        let mut is_float = false;
        self.digits(&mut text);
        if self.peek() == Some('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            is_float = true;
            text.push('.');
            self.bump();
            self.digits(&mut text);
        }
        if matches!(self.peek(), Some('e') | Some('E')) {
            let sign = self.peek_at(1);
            let has_digits = match sign {
                Some('+') | Some('-') => self.peek_at(2).is_some_and(|c| c.is_ascii_digit()),
                Some(c) => c.is_ascii_digit(),
                None => false,
            };
            if has_digits {
                is_float = true;
                text.push('e');
                self.bump();
                if let Some(s @ ('+' | '-')) = self.peek() {
                    text.push(s);
                    self.bump();
                }
                self.digits(&mut text);
            }
        }
        if is_float {
            return text
                .parse::<f64>()
                .map(Tok::Float)
                .map_err(|_| self.err(span, "некорректный вещественный литерал"));
        }
        match text.parse::<u64>() {
            // 9223372036854775808 is allowed so that `-9223372036854775808` works.
            Ok(v) if v <= i64::MAX as u64 + 1 => Ok(Tok::Int(v as i64)),
            _ => Err(self.err(span, "целочисленный литерал не помещается в int (64 бита)")),
        }
    }

    fn digits(&mut self, out: &mut String) {
        while let Some(c) = self.peek() {
            if c == '_' {
                self.bump();
            } else if c.is_ascii_digit() {
                out.push(c);
                self.bump();
            } else {
                break;
            }
        }
    }

    fn escape(&mut self, span: Span) -> Result<char, Diag> {
        Ok(match self.bump() {
            Some('n') => '\n',
            Some('t') => '\t',
            Some('r') => '\r',
            Some('0') => '\0',
            Some('\\') => '\\',
            Some('"') => '"',
            Some('\'') => '\'',
            Some('{') => '{',
            Some('}') => '}',
            Some('u') => {
                if self.bump() != Some('{') {
                    return Err(self.err(span, "ожидалось \\u{XXXX}"));
                }
                let mut hex = String::new();
                while let Some(c) = self.peek() {
                    if c == '}' {
                        break;
                    }
                    hex.push(c);
                    self.bump();
                }
                self.bump();
                u32::from_str_radix(&hex, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| {
                        self.err(span, format!("некорректный код символа \\u{{{hex}}}"))
                    })?
            }
            Some(c) => {
                return Err(self.err(
                    self.span(),
                    format!("неизвестная escape-последовательность '\\{c}'"),
                ))
            }
            None => return Err(self.err(span, "незакрытая строка")),
        })
    }

    fn string(&mut self, span: Span) -> Result<Vec<StrPart>, Diag> {
        let mut parts = Vec::new();
        let mut lit = String::new();
        loop {
            match self.bump() {
                None => return Err(self.err(span, "незакрытая строка")),
                Some('"') => break,
                Some('\\') => lit.push(self.escape(span)?),
                Some('{') => {
                    if !lit.is_empty() {
                        parts.push(StrPart::Lit(std::mem::take(&mut lit)));
                    }
                    let hole = self.span();
                    let mut toks = Vec::new();
                    let mut depth = 0usize;
                    loop {
                        let t = self.next_token()?;
                        match t.tok {
                            Tok::Eof => {
                                return Err(self.err(hole, "незакрытая интерполяция '{' в строке"))
                            }
                            Tok::LBrace => depth += 1,
                            Tok::RBrace if depth == 0 => {
                                toks.push(Token {
                                    tok: Tok::Eof,
                                    span: t.span,
                                });
                                break;
                            }
                            Tok::RBrace => depth -= 1,
                            _ => {}
                        }
                        toks.push(t);
                    }
                    if toks.len() == 1 {
                        return Err(self.err(
                            hole,
                            "пустая интерполяция {} (для фигурной скобки используйте \\{)",
                        ));
                    }
                    parts.push(StrPart::Expr(toks));
                }
                Some('}') => {
                    return Err(self.err(self.span(), "одиночная '}' в строке (используйте \\})"))
                }
                Some(c) => lit.push(c),
            }
        }
        if !lit.is_empty() || parts.is_empty() {
            parts.push(StrPart::Lit(lit));
        }
        Ok(parts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        tokenize(src, 0)
            .unwrap()
            .tokens
            .into_iter()
            .map(|t| t.tok)
            .collect()
    }

    #[test]
    fn numbers_and_ranges() {
        assert_eq!(
            toks("0..10 1.5 0xff 1_000 2e3"),
            vec![
                Tok::Int(0),
                Tok::DotDot,
                Tok::Int(10),
                Tok::Float(1.5),
                Tok::Int(255),
                Tok::Int(1000),
                Tok::Float(2000.0),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn interpolation() {
        let t = toks("\"a{x + 1}b\"");
        match &t[0] {
            Tok::Str(parts) => {
                assert_eq!(parts.len(), 3);
                assert_eq!(parts[0], StrPart::Lit("a".into()));
                assert!(matches!(&parts[1], StrPart::Expr(v) if v.len() == 4));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn comments_are_collected() {
        let l = tokenize("// a\nlet x = 1; // b\n", 0).unwrap();
        assert_eq!(l.comments.len(), 2);
        assert!(l.comments[0].own_line);
        assert!(!l.comments[1].own_line);
    }
}
