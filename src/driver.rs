//! Compilation pipeline: load sources (following imports) → check → back end.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{env, fs};

use crate::ast::{Item, ItemKind};
use crate::check::{self, Module};
use crate::codegen;
use crate::diag::{Diag, SourceMap, Span};
use crate::lexer::tokenize;
use crate::parser::parse_program;

const RUNTIME_C: &str = include_str!("../runtime/orbitron_rt.c");
const RUNTIME_JAVA: &str = include_str!("../runtime/OtRt.java");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Llvm,
    Jvm,
}

impl Backend {
    pub fn parse(s: &str) -> Option<Backend> {
        match s {
            "llvm" | "native" => Some(Backend::Llvm),
            "jvm" | "java" => Some(Backend::Jvm),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub backend: Backend,
    /// Output path. For the JVM back end `.jar` is appended if missing.
    pub output: PathBuf,
    /// Stop after writing `.ll` / `.java` next to the output.
    pub emit_source: bool,
    pub keep_temps: bool,
    pub verbose: bool,
    pub opt_level: u8,
}

// ── Stdlib discovery ──────────────────────────────────────────────────────────

pub fn find_stdlib() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(home) = env::var("ORBITRON_HOME") {
        candidates.push(PathBuf::from(home).join("stdlib"));
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("stdlib"));
            candidates.push(dir.join("../share/orbitron/stdlib"));
            candidates.push(dir.join("../../stdlib")); // target/{debug,release}
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("stdlib"));
    candidates.into_iter().find(|p| p.is_dir())
}

// ── Loading ───────────────────────────────────────────────────────────────────

pub struct Loaded {
    pub sources: SourceMap,
    pub items: Vec<Item>,
    /// Short file names used in runtime error locations.
    pub short_names: Vec<String>,
}

/// Reads `entry` and everything it imports. Errors are returned already
/// rendered with source context.
pub fn load(entry: &Path) -> Result<Loaded, String> {
    let mut l = Loader {
        sources: SourceMap::default(),
        items: Vec::new(),
        short_names: Vec::new(),
        visited: HashSet::new(),
        stdlib: find_stdlib(),
    };
    match l.file(entry, None) {
        Ok(()) => Ok(Loaded {
            sources: l.sources,
            items: l.items,
            short_names: l.short_names,
        }),
        Err(d) => Err(l.sources.render_all(&[d])),
    }
}

struct Loader {
    sources: SourceMap,
    items: Vec<Item>,
    short_names: Vec<String>,
    visited: HashSet<PathBuf>,
    stdlib: Option<PathBuf>,
}

impl Loader {
    fn file(&mut self, path: &Path, from: Option<Span>) -> Result<(), Diag> {
        let canonical = fs::canonicalize(path).map_err(|e| {
            Diag::new(
                from.unwrap_or_default(),
                format!("не удалось открыть '{}': {e}", path.display()),
            )
        })?;
        if !self.visited.insert(canonical) {
            return Ok(());
        }
        let text = fs::read_to_string(path).map_err(|e| {
            Diag::new(
                from.unwrap_or_default(),
                format!("не удалось прочитать '{}': {e}", path.display()),
            )
        })?;
        let text = text.replace("\r\n", "\n");
        let display = path.display().to_string();
        let short = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| display.clone());
        let id = self.sources.add(display, text.clone());
        self.short_names.push(short);

        let lexed = tokenize(&text, id)?;
        let items = parse_program(lexed.tokens)?;
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        for item in items {
            if let ItemKind::Import(name) = &item.kind {
                let target = self.resolve_import(name, &dir, item.span)?;
                self.file(&target, Some(item.span))?;
            }
            self.items.push(item);
        }
        Ok(())
    }

    fn resolve_import(&self, name: &str, dir: &Path, span: Span) -> Result<PathBuf, Diag> {
        let rel = if name.ends_with(".ot") {
            name.to_string()
        } else {
            format!("{name}.ot")
        };
        if let Some(lib) = rel.strip_prefix("std/") {
            let root = self.stdlib.as_ref().ok_or_else(|| {
                Diag::new(span, "стандартная библиотека не найдена").with_note(
                    "укажите каталог через ORBITRON_HOME (в нём должна быть папка stdlib/)",
                )
            })?;
            let p = root.join(lib);
            if !p.exists() {
                return Err(Diag::new(
                    span,
                    format!("в стандартной библиотеке нет модуля '{name}'"),
                ));
            }
            return Ok(p);
        }
        let p = dir.join(&rel);
        if !p.exists() {
            return Err(Diag::new(
                span,
                format!("модуль '{name}' не найден (искали {})", p.display()),
            ));
        }
        Ok(p)
    }
}

/// Loads and type-checks a program.
pub fn check_program(entry: &Path) -> Result<(Module, Vec<String>), String> {
    let loaded = load(entry)?;
    match check::check(loaded.items, true) {
        Ok(m) => Ok((m, loaded.short_names)),
        Err(diags) => Err(loaded.sources.render_all(&diags)),
    }
}

// ── Building ──────────────────────────────────────────────────────────────────

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> Result<PathBuf, String> {
    let n = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = env::temp_dir().join(format!("orbitron-{}-{n}", std::process::id()));
    fs::create_dir_all(&dir).map_err(|e| format!("не удалось создать временный каталог: {e}"))?;
    Ok(dir)
}

fn run_tool(cmd: &mut Command, tool: &str, hint: &str, verbose: bool) -> Result<(), String> {
    if verbose {
        eprintln!("  $ {:?}", cmd);
    }
    let out = cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("не найдена программа '{tool}'. {hint}")
        } else {
            format!("не удалось запустить '{tool}': {e}")
        }
    })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        return Err(format!(
            "внутренняя ошибка: '{tool}' завершился с ошибкой (это ошибка компилятора Orbitron)\n{stdout}{stderr}"
        ));
    }
    Ok(())
}

/// Compiles `entry` and returns the path of the produced artifact.
pub fn build(entry: &Path, opts: &BuildOptions) -> Result<PathBuf, String> {
    if opts.verbose {
        eprintln!("[1/3] разбор и проверка типов: {}", entry.display());
    }
    let (module, names) = check_program(entry)?;
    if let Some(parent) = opts.output.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("не удалось создать каталог '{}': {e}", parent.display()))?;
        }
    }
    match opts.backend {
        Backend::Llvm => build_llvm(&module, &names, opts),
        Backend::Jvm => build_jvm(&module, &names, opts),
    }
}

fn build_llvm(module: &Module, names: &[String], opts: &BuildOptions) -> Result<PathBuf, String> {
    if opts.verbose {
        eprintln!("[2/3] генерация LLVM IR");
    }
    let ir = codegen::llvm::emit(module, names);
    if opts.emit_source {
        let path = opts.output.with_extension("ll");
        fs::write(&path, ir)
            .map_err(|e| format!("не удалось записать '{}': {e}", path.display()))?;
        return Ok(path);
    }
    let dir = temp_dir()?;
    let ll = dir.join("program.ll");
    let rt = dir.join("orbitron_rt.c");
    fs::write(&ll, ir).map_err(|e| e.to_string())?;
    fs::write(&rt, RUNTIME_C).map_err(|e| e.to_string())?;
    if opts.verbose {
        eprintln!("[3/3] clang → {}", opts.output.display());
    }
    let clang = env::var("ORBITRON_CC").unwrap_or_else(|_| "clang".into());
    let result = run_tool(
        Command::new(&clang)
            .arg(format!("-O{}", opts.opt_level))
            .arg("-w")
            .arg(&ll)
            .arg(&rt)
            .arg("-o")
            .arg(&opts.output)
            .arg("-lm"),
        &clang,
        "Для LLVM-бэкенда нужен clang (macOS: xcode-select --install; Ubuntu: sudo apt install clang).",
        opts.verbose,
    );
    finish_temps(&dir, opts);
    result.map(|_| opts.output.clone())
}

fn build_jvm(module: &Module, names: &[String], opts: &BuildOptions) -> Result<PathBuf, String> {
    if let Some(ex) = module.externs.first() {
        return Err(format!(
            "ошибка: внешняя функция '{}' недоступна в JVM-бэкенде\n  = примечание: extern fn поддерживается только LLVM-бэкендом",
            ex.name
        ));
    }
    if opts.verbose {
        eprintln!("[2/3] генерация Java");
    }
    let java = codegen::jvm::emit(module, names);
    if opts.emit_source {
        let path = opts.output.with_extension("java");
        fs::write(&path, java)
            .map_err(|e| format!("не удалось записать '{}': {e}", path.display()))?;
        return Ok(path);
    }
    let dir = temp_dir()?;
    let classes = dir.join("classes");
    fs::create_dir_all(&classes).map_err(|e| e.to_string())?;
    fs::write(dir.join("Main.java"), java).map_err(|e| e.to_string())?;
    fs::write(dir.join("OtRt.java"), RUNTIME_JAVA).map_err(|e| e.to_string())?;
    let jar = if opts.output.extension().is_some_and(|e| e == "jar") {
        opts.output.clone()
    } else {
        let mut s = opts.output.clone().into_os_string();
        s.push(".jar");
        PathBuf::from(s)
    };
    if opts.verbose {
        eprintln!("[3/3] javac + jar → {}", jar.display());
    }
    let hint = "Для JVM-бэкенда нужен JDK 17+ (javac и jar в PATH).";
    let result = run_tool(
        Command::new("javac")
            .args(["-encoding", "UTF-8", "-nowarn", "-Xlint:none", "-d"])
            .arg(&classes)
            .arg(dir.join("Main.java"))
            .arg(dir.join("OtRt.java")),
        "javac",
        hint,
        opts.verbose,
    )
    .and_then(|_| {
        let _ = fs::remove_file(&jar);
        run_tool(
            Command::new("jar")
                .arg("cfe")
                .arg(&jar)
                .arg("Main")
                .arg("-C")
                .arg(&classes)
                .arg("."),
            "jar",
            hint,
            opts.verbose,
        )
    });
    finish_temps(&dir, opts);
    result.map(|_| jar)
}

fn finish_temps(dir: &Path, opts: &BuildOptions) {
    if opts.keep_temps {
        eprintln!("временные файлы: {}", dir.display());
    } else {
        let _ = fs::remove_dir_all(dir);
    }
}

/// Runs a built artifact, forwarding the exit code.
pub fn run_artifact(path: &Path, backend: Backend) -> Result<i32, String> {
    let status = match backend {
        Backend::Llvm => {
            let p = if path.is_relative() && path.components().count() == 1 {
                Path::new(".").join(path)
            } else {
                path.to_path_buf()
            };
            Command::new(&p)
                .status()
                .map_err(|e| format!("не удалось запустить '{}': {e}", p.display()))?
        }
        Backend::Jvm => Command::new("java")
            .arg("-Xss1g")
            .arg("-jar")
            .arg(path)
            .status()
            .map_err(|e| format!("не удалось запустить java: {e}"))?,
    };
    Ok(status.code().unwrap_or(1))
}

pub fn temp_output(stem: &str) -> Result<PathBuf, String> {
    Ok(temp_dir()?.join(stem))
}
