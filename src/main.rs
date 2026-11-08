//! Orbitron compiler — command-line interface.

mod ast;
mod check;
mod codegen;
mod diag;
mod driver;
mod fmt;
mod lexer;
mod parser;
mod project;
mod types;

use std::path::{Path, PathBuf};
use std::process;

use driver::{Backend, BuildOptions};

const VERSION: &str = env!("CARGO_PKG_VERSION");

const HELP: &str = "\
Orbitron — компилятор языка Orbitron

ИСПОЛЬЗОВАНИЕ:
    orbitron run [файл.ot] [опции]     собрать и запустить
    orbitron build [файл.ot] [опции]   собрать исполняемый файл
    orbitron check [файл.ot]           только проверить код (без сборки)
    orbitron fmt [--check] [файлы...]  отформатировать исходники
    orbitron new <имя>                 создать новый проект
    orbitron <файл.ot> [опции]         то же, что build

Без файла команды run/build/check работают с проектом (orbitron.toml).

ОПЦИИ СБОРКИ:
    -o <путь>              путь к результату
    --backend <llvm|jvm>   бэкенд: нативный код через LLVM (по умолчанию) или JVM
    --emit                 только сгенерировать LLVM IR (.ll) или Java (.java)
    -O<0-3>                уровень оптимизации LLVM (по умолчанию -O2)
    --keep-temps           не удалять промежуточные файлы
    -v, --verbose          подробный вывод

ОПЦИИ FMT:
    --check                не менять файлы, вернуть код 1, если нужно форматирование
    --stdout               вывести результат вместо записи в файл

    -h, --help             эта справка
    --version              версия компилятора
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match dispatch(&args) {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("{msg}");
            1
        }
    };
    process::exit(code);
}

fn dispatch(args: &[String]) -> Result<i32, String> {
    let Some(cmd) = args.first() else {
        print!("{HELP}");
        return Ok(0);
    };
    match cmd.as_str() {
        "-h" | "--help" | "help" => {
            print!("{HELP}");
            Ok(0)
        }
        "--version" | "-V" | "version" => {
            println!("orbitron {VERSION}");
            Ok(0)
        }
        "new" => cmd_new(&args[1..]),
        "fmt" => cmd_fmt(&args[1..]),
        "check" => cmd_check(&args[1..]),
        "build" => cmd_build(&args[1..], false),
        "run" => cmd_build(&args[1..], true),
        _ => cmd_build(args, false),
    }
}

// ── Option parsing ────────────────────────────────────────────────────────────

struct Cli {
    file: Option<PathBuf>,
    output: Option<PathBuf>,
    backend: Option<Backend>,
    emit: bool,
    keep_temps: bool,
    verbose: bool,
    opt_level: u8,
}

fn parse_cli(args: &[String]) -> Result<Cli, String> {
    let mut c = Cli {
        file: None,
        output: None,
        backend: None,
        emit: false,
        keep_temps: false,
        verbose: false,
        opt_level: 2,
    };
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-o" | "--output" => {
                i += 1;
                c.output = Some(PathBuf::from(args.get(i).ok_or("после -o нужен путь")?));
            }
            "--backend" | "-b" => {
                i += 1;
                let v = args
                    .get(i)
                    .ok_or("после --backend нужно указать llvm или jvm")?;
                c.backend =
                    Some(Backend::parse(v).ok_or_else(|| {
                        format!("неизвестный бэкенд '{v}' (допустимы llvm, jvm)")
                    })?);
            }
            "--emit" | "--emit-llvm" | "--emit-java" => c.emit = true,
            "--keep-temps" | "--save-temps" => c.keep_temps = true,
            "-v" | "--verbose" => c.verbose = true,
            "-O0" | "-O1" | "-O2" | "-O3" => c.opt_level = a[2..].parse().unwrap(),
            _ if a.starts_with("--backend=") => {
                let v = &a["--backend=".len()..];
                c.backend =
                    Some(Backend::parse(v).ok_or_else(|| {
                        format!("неизвестный бэкенд '{v}' (допустимы llvm, jvm)")
                    })?);
            }
            _ if a.starts_with('-') => {
                return Err(format!("неизвестная опция '{a}' (см. orbitron --help)"))
            }
            _ => {
                if c.file.is_some() {
                    return Err("можно указать только один входной файл; для нескольких модулей используйте import".into());
                }
                c.file = Some(PathBuf::from(a));
            }
        }
        i += 1;
    }
    Ok(c)
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Entry file, output path and backend for the current invocation.
fn resolve_target(cli: &Cli, for_run: bool) -> Result<(PathBuf, PathBuf, Backend), String> {
    if let Some(file) = &cli.file {
        if !file.exists() {
            return Err(format!("файл '{}' не найден", file.display()));
        }
        let backend = cli.backend.unwrap_or(Backend::Llvm);
        let stem = file
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "a.out".into());
        let output = match &cli.output {
            Some(o) => o.clone(),
            None if for_run => driver::temp_output(&stem)?,
            None => PathBuf::from(&stem),
        };
        return Ok((file.clone(), output, backend));
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let root = project::find_root(&cwd).ok_or(
        "не указан файл и не найден orbitron.toml\n  = примечание: запустите 'orbitron run файл.ot' или создайте проект: 'orbitron new имя'",
    )?;
    let m = project::load(&root)?;
    let backend = match cli.backend {
        Some(b) => b,
        None => Backend::parse(&m.build.backend)
            .ok_or_else(|| format!("orbitron.toml: неизвестный бэкенд '{}'", m.build.backend))?,
    };
    let output = cli
        .output
        .clone()
        .unwrap_or_else(|| root.join(m.output_path()));
    Ok((root.join(&m.build.main), output, backend))
}

fn cmd_build(args: &[String], run_after: bool) -> Result<i32, String> {
    let cli = parse_cli(args)?;
    let (entry, output, backend) = resolve_target(&cli, run_after)?;
    let opts = BuildOptions {
        backend,
        output,
        emit_source: cli.emit,
        keep_temps: cli.keep_temps,
        verbose: cli.verbose,
        opt_level: cli.opt_level,
    };
    let artifact = driver::build(&entry, &opts)?;
    if cli.emit {
        println!("записано: {}", artifact.display());
        return Ok(0);
    }
    if !run_after {
        if cli.verbose || cli.file.is_none() {
            println!("собрано: {}", artifact.display());
        }
        return Ok(0);
    }
    let code = driver::run_artifact(&artifact, backend)?;
    if cli.file.is_some() && cli.output.is_none() {
        if let Some(dir) = artifact.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
    Ok(code)
}

fn cmd_check(args: &[String]) -> Result<i32, String> {
    let cli = parse_cli(args)?;
    let (entry, _, _) = resolve_target(&cli, false)?;
    driver::check_program(&entry)?;
    println!("ошибок не найдено: {}", entry.display());
    Ok(0)
}

fn cmd_new(args: &[String]) -> Result<i32, String> {
    let name = args.first().ok_or("использование: orbitron new <имя>")?;
    project::create(Path::new(name))?;
    println!("Создан проект '{name}'. Дальше:\n    cd {name}\n    orbitron run");
    Ok(0)
}

fn cmd_fmt(args: &[String]) -> Result<i32, String> {
    let mut check = false;
    let mut to_stdout = false;
    let mut files: Vec<PathBuf> = Vec::new();
    for a in args {
        match a.as_str() {
            "--check" => check = true,
            "--stdout" => to_stdout = true,
            _ if a.starts_with('-') => return Err(format!("неизвестная опция fmt '{a}'")),
            _ => files.push(PathBuf::from(a)),
        }
    }
    if files.is_empty() {
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        let root = project::find_root(&cwd).unwrap_or(cwd);
        collect_ot(&root, &mut files)?;
        files.sort();
    }
    let mut unformatted = 0;
    for f in &files {
        let src = std::fs::read_to_string(f)
            .map_err(|e| format!("не удалось прочитать '{}': {e}", f.display()))?;
        let formatted = fmt::format_source(&src.replace("\r\n", "\n")).map_err(|d| {
            let mut sm = diag::SourceMap::default();
            sm.add(f.display().to_string(), src.clone());
            sm.render(&d)
        })?;
        if to_stdout {
            print!("{formatted}");
        } else if formatted != src {
            unformatted += 1;
            if check {
                println!("требует форматирования: {}", f.display());
            } else {
                std::fs::write(f, &formatted)
                    .map_err(|e| format!("не удалось записать '{}': {e}", f.display()))?;
                println!("отформатирован: {}", f.display());
            }
        }
    }
    Ok(if check && unformatted > 0 { 1 } else { 0 })
}

fn collect_ot(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("не удалось прочитать каталог '{}': {e}", dir.display()))?;
    for e in entries.flatten() {
        let p = e.path();
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if name.starts_with('.') || name == "target" || name == "build" {
            continue;
        }
        if p.is_dir() {
            collect_ot(&p, out)?;
        } else if p.extension().is_some_and(|x| x == "ot") {
            out.push(p);
        }
    }
    Ok(())
}
