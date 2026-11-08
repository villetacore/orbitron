//! Projects: `orbitron.toml` manifest and `orbitron new`.

use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
pub struct Manifest {
    pub project: ProjectSection,
    #[serde(default)]
    pub build: BuildSection,
}

#[derive(Deserialize)]
pub struct ProjectSection {
    pub name: String,
    #[allow(dead_code)]
    #[serde(default = "default_version")]
    pub version: String,
}

#[derive(Deserialize)]
pub struct BuildSection {
    #[serde(default = "default_main")]
    pub main: String,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default = "default_backend")]
    pub backend: String,
}

impl Default for BuildSection {
    fn default() -> Self {
        BuildSection {
            main: default_main(),
            output: None,
            backend: default_backend(),
        }
    }
}

fn default_version() -> String {
    "0.1.0".into()
}
fn default_main() -> String {
    "src/main.ot".into()
}
fn default_backend() -> String {
    "llvm".into()
}

impl Manifest {
    pub fn output_path(&self) -> PathBuf {
        match &self.build.output {
            Some(o) => PathBuf::from(o),
            None => PathBuf::from("build").join(&self.project.name),
        }
    }
}

pub fn find_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join("orbitron.toml").is_file() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

pub fn load(root: &Path) -> Result<Manifest, String> {
    let path = root.join("orbitron.toml");
    let text = fs::read_to_string(&path)
        .map_err(|e| format!("не удалось прочитать {}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("ошибка в {}: {e}", path.display()))
}

pub fn create(root: &Path) -> Result<(), String> {
    if root.exists() {
        return Err(format!("каталог '{}' уже существует", root.display()));
    }
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or("некорректное имя проекта")?;
    fs::create_dir_all(root.join("src")).map_err(|e| format!("не удалось создать каталог: {e}"))?;
    let manifest = format!(
        "[project]\nname = \"{name}\"\nversion = \"0.1.0\"\n\n[build]\nmain = \"src/main.ot\"\nbackend = \"llvm\"   # или \"jvm\"\n"
    );
    fs::write(root.join("orbitron.toml"), manifest).map_err(|e| e.to_string())?;
    let main = format!("fn main() {{\n    println(\"Привет из {name}!\");\n}}\n");
    fs::write(root.join("src/main.ot"), main).map_err(|e| e.to_string())?;
    fs::write(root.join(".gitignore"), "/build/\n").map_err(|e| e.to_string())?;
    Ok(())
}
