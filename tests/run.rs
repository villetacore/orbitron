//! End-to-end tests: every program in tests/programs is compiled with both
//! back ends and its output is compared with the `.out` file next to it.
//! Programs in tests/errors must fail to compile with the listed messages.
//!
//! The JVM back end is skipped when `javac` is not installed or
//! ORBITRON_SKIP_JVM is set.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn compiler() -> &'static str {
    env!("CARGO_BIN_EXE_orbitron")
}

fn files(dir: &str) -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(dir);
    let mut v: Vec<PathBuf> = fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "ot"))
        .collect();
    v.sort();
    v
}

fn header(src: &str, key: &str) -> Vec<String> {
    let prefix = format!("// {key}:");
    src.lines()
        .filter_map(|l| l.strip_prefix(&prefix).map(|s| s.trim().to_string()))
        .collect()
}

fn jvm_available() -> bool {
    std::env::var_os("ORBITRON_SKIP_JVM").is_none()
        && Command::new("javac")
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success())
}

fn run_program(path: &Path, backend: &str) -> Result<(), String> {
    let src = fs::read_to_string(path).unwrap();
    let expected_out = fs::read_to_string(path.with_extension("out")).unwrap_or_default();
    let expected_code: i32 = header(&src, "exit")
        .first()
        .map(|s| s.parse().unwrap())
        .unwrap_or(0);
    let expected_err = header(&src, "stderr");

    let out = Command::new(compiler())
        .arg("run")
        .arg(path)
        .arg("--backend")
        .arg(backend)
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let name = path.file_name().unwrap().to_string_lossy();
    if stdout != expected_out {
        return Err(format!("[{backend}] {name}: вывод отличается\n--- ожидалось\n{expected_out}--- получено\n{stdout}--- stderr\n{stderr}"));
    }
    if out.status.code() != Some(expected_code) {
        return Err(format!(
            "[{backend}] {name}: код {:?}, ожидался {expected_code}\n{stderr}",
            out.status.code()
        ));
    }
    for e in &expected_err {
        if !stderr.contains(e.as_str()) {
            return Err(format!("[{backend}] {name}: в stderr нет «{e}»\n{stderr}"));
        }
    }
    Ok(())
}

#[test]
fn programs_llvm() {
    let failures: Vec<String> = files("tests/programs")
        .iter()
        .filter_map(|p| run_program(p, "llvm").err())
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn programs_jvm() {
    if !jvm_available() {
        eprintln!("javac не найден — JVM-тесты пропущены");
        return;
    }
    let failures: Vec<String> = files("tests/programs")
        .iter()
        .filter_map(|p| run_program(p, "jvm").err())
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn examples_compile() {
    let failures: Vec<String> = files("examples")
        .iter()
        .filter_map(|p| {
            let out = Command::new(compiler())
                .arg("check")
                .arg(p)
                .output()
                .unwrap();
            if out.status.success() {
                None
            } else {
                Some(format!(
                    "{}:\n{}",
                    p.display(),
                    String::from_utf8_lossy(&out.stderr)
                ))
            }
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn compile_errors() {
    let mut failures = Vec::new();
    for p in files("tests/errors") {
        let src = fs::read_to_string(&p).unwrap();
        let out = Command::new(compiler())
            .arg("check")
            .arg(&p)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        if out.status.success() {
            failures.push(format!(
                "{}: компиляция должна была завершиться ошибкой",
                p.display()
            ));
            continue;
        }
        for e in header(&src, "ошибка") {
            if !stderr.contains(&format!("ошибка: {e}")) {
                failures.push(format!("{}: нет ошибки «{e}»\n{stderr}", p.display()));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn fmt_is_idempotent_on_sources() {
    let mut all = files("examples");
    all.extend(files("stdlib"));
    let mut failures = Vec::new();
    for p in all {
        let out = Command::new(compiler())
            .arg("fmt")
            .arg("--check")
            .arg(&p)
            .output()
            .unwrap();
        if !out.status.success() {
            failures.push(p.display().to_string());
        }
    }
    assert!(
        failures.is_empty(),
        "не отформатированы: {}",
        failures.join(", ")
    );
}
