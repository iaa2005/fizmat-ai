// Fizmat Code на компьютере: привязка к аккаунту, папки, которые человек выбрал
// в окне программы, и исполнение вызовов модели (дерево, чтение, поиск).
//
// Программа сама спрашивает у fizmat.ai вызовы (GET /api/code/calls) ключом
// устройства и приносит ответы. Всё исполняется только внутри выбранных папок:
// путь вызова приводится к настоящему и сверяется с корнем.

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_FILE: u64 = 20 * 1024 * 1024;
const MAX_SEARCH_FILE: u64 = 2 * 1024 * 1024;
const MAX_LINE: usize = 2000;
const LIST_LIMIT: usize = 600;

fn site() -> String {
    std::env::var("FIZMAT_SITE").unwrap_or_else(|_| "https://fizmat.ai".into())
}

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct Root {
    pub path: String,
    pub label: String,
}

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct Config {
    pub device_id: Option<i64>,
    pub token: Option<String>,
    #[serde(default)]
    pub roots: Vec<Root>,
}

pub struct Code {
    pub cfg: Mutex<Config>,
    polling: AtomicBool,
}

fn file() -> PathBuf {
    crate::home().join("code.json")
}

impl Code {
    pub fn load() -> Self {
        let cfg = fs::read_to_string(file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Code { cfg: Mutex::new(cfg), polling: AtomicBool::new(false) }
    }

    fn save(&self) {
        let cfg = self.cfg.lock().unwrap().clone();
        let _ = fs::create_dir_all(crate::home());
        let _ = fs::write(file(), serde_json::to_string_pretty(&cfg).unwrap_or_default());
    }

    fn token(&self) -> Option<String> {
        self.cfg.lock().unwrap().token.clone()
    }

    /// Сказать сайту версию и папки. 401 — компьютер отвязали на сайте.
    fn hello(&self) -> Result<(), String> {
        let Some(token) = self.token() else { return Err("не привязан".into()) };
        let roots = self.cfg.lock().unwrap().roots.clone();
        let res = ureq::post(&format!("{}/api/code/hello", site()))
            .set("Authorization", &format!("Bearer {token}"))
            .set("Accept", "application/json")
            .timeout(Duration::from_secs(15))
            .send_json(json!({ "version": VERSION, "platform": std::env::consts::OS, "roots": roots }));
        match res {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(401, _)) => {
                self.unpair();
                Err("компьютер отвязан на сайте".into())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    fn unpair(&self) {
        let mut cfg = self.cfg.lock().unwrap();
        cfg.token = None;
        cfg.device_id = None;
        drop(cfg);
        self.save();
    }
}

fn computer_name() -> String {
    std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "Компьютер".into())
}

fn status(code: &Code) -> Value {
    let cfg = code.cfg.lock().unwrap();
    json!({ "paired": cfg.token.is_some(), "device_id": cfg.device_id, "name": computer_name(), "roots": cfg.roots, "version": VERSION })
}

// ── Команды страницы fizmat.ai (раздел Code) ─────────────────────────────

#[tauri::command]
pub fn code_status(code: State<Code>) -> Value {
    status(&code)
}

/// Ключ, выданный сайтом при «Подключить этот компьютер».
#[tauri::command]
pub fn code_pair(id: i64, token: String, app: AppHandle, code: State<Code>) -> Result<Value, String> {
    if !token.starts_with("fzd_") || token.len() > 200 {
        return Err("неверный ключ".into());
    }
    {
        let mut cfg = code.cfg.lock().unwrap();
        cfg.device_id = Some(id);
        cfg.token = Some(token);
    }
    code.save();
    code.hello()?;
    start(app);
    Ok(status(&code))
}

/// Папку выбирает человек в окне программы — страница путь не задаёт.
#[tauri::command]
pub async fn code_add_root(app: AppHandle, code: State<'_, Code>) -> Result<Option<Value>, String> {
    let picked = app.dialog().file().set_title("Папка для Fizmat Code").blocking_pick_folder();
    let Some(picked) = picked else { return Ok(None) };
    let path = picked.into_path().map_err(|e| e.to_string())?;
    let path = path.to_string_lossy().to_string();
    let label = Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.clone());
    {
        let mut cfg = code.cfg.lock().unwrap();
        if !cfg.roots.iter().any(|r| r.path == path) {
            cfg.roots.push(Root { path, label });
        }
    }
    code.save();
    let _ = code.hello();
    Ok(Some(status(&code)))
}

#[tauri::command]
pub fn code_remove_root(path: String, code: State<Code>) -> Result<Value, String> {
    code.cfg.lock().unwrap().roots.retain(|r| r.path != path);
    code.save();
    let _ = code.hello();
    Ok(status(&code))
}

// ── Опрос сайта ──────────────────────────────────────────────────────────

/// Фоновый опрос: раз в секунду, пока недавно были вызовы, иначе раз в три.
pub fn start(app: AppHandle) {
    let code = app.state::<Code>();
    if code.polling.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let code = app.state::<Code>();
        let mut last_call = Instant::now();
        let mut last_hello = Instant::now() - Duration::from_secs(3600);
        loop {
            let Some(token) = code.token() else {
                std::thread::sleep(Duration::from_secs(3));
                continue;
            };
            if last_hello.elapsed() > Duration::from_secs(300) && code.hello().is_ok() {
                last_hello = Instant::now();
            }
            let res = ureq::get(&format!("{}/api/code/calls", site()))
                .set("Authorization", &format!("Bearer {token}"))
                .set("Accept", "application/json")
                .timeout(Duration::from_secs(15))
                .call();
            match res {
                Ok(r) => {
                    let calls = r.into_json::<Value>().ok().and_then(|v| v["calls"].as_array().cloned()).unwrap_or_default();
                    if !calls.is_empty() {
                        last_call = Instant::now();
                    }
                    for call in calls {
                        let answer = execute(&code, &call);
                        let id = call["id"].as_i64().unwrap_or(0);
                        let _ = ureq::post(&format!("{}/api/code/calls/{id}", site()))
                            .set("Authorization", &format!("Bearer {token}"))
                            .set("Accept", "application/json")
                            .timeout(Duration::from_secs(30))
                            .send_json(answer);
                    }
                }
                Err(ureq::Error::Status(401, _)) => code.unpair(),
                Err(_) => std::thread::sleep(Duration::from_secs(5)),
            }
            let pause = if last_call.elapsed() < Duration::from_secs(120) { 1 } else { 3 };
            std::thread::sleep(Duration::from_secs(pause));
        }
    });
}

// ── Исполнение ───────────────────────────────────────────────────────────

fn fail(msg: impl Into<String>) -> Value {
    json!({ "ok": false, "error": msg.into() })
}

fn execute(code: &Code, call: &Value) -> Value {
    let root = call["root"].as_str().unwrap_or("");
    // Корень — только из своих папок: сайт не может назвать чужую.
    if !code.cfg.lock().unwrap().roots.iter().any(|r| r.path == root) {
        return fail("этой папки нет в программе на компьютере");
    }
    let args = &call["args"];
    let result = match call["tool"].as_str().unwrap_or("") {
        "list" => list(root, args),
        "read" => read(root, args),
        "search" => search(root, args),
        _ => Err("неизвестный вызов".into()),
    };
    result.unwrap_or_else(fail)
}

/// Путь внутри корня: без абсолютных путей и «..», настоящий путь — под корнем.
fn inside(root: &str, rel: &str) -> Result<(PathBuf, PathBuf), String> {
    let base = fs::canonicalize(root).map_err(|_| "папки нет на компьютере".to_string())?;
    let rel = rel.trim();
    let rel_path = Path::new(if rel.is_empty() { "." } else { rel });
    if rel_path.is_absolute() || rel.contains(':') || rel_path.components().any(|c| matches!(c, Component::ParentDir | Component::Prefix(_) | Component::RootDir)) {
        return Err("путь должен быть внутри папки, без «..»".into());
    }
    let full = fs::canonicalize(base.join(rel_path)).map_err(|_| format!("нет такого: {rel}"))?;
    if !full.starts_with(&base) {
        return Err("путь ведёт за пределы папки".into());
    }
    Ok((base, full))
}

fn shown(base: &Path, p: &Path) -> String {
    let rel = p.strip_prefix(base).unwrap_or(p).to_string_lossy().replace('\\', "/");
    if rel.is_empty() { ".".into() } else { rel }
}

/// Секреты модель не читает (до разрешения в окне программы — потом).
fn secret(p: &Path) -> bool {
    let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    name.starts_with(".env") || name.starts_with("id_rsa") || name.starts_with("id_ed25519") || name.starts_with("id_ecdsa")
        || [".pem", ".key", ".pfx", ".p12", ".keystore", ".jks"].iter().any(|ext| name.ends_with(ext))
}

fn walker(dir: &Path, depth: Option<usize>) -> WalkBuilder {
    let mut w = WalkBuilder::new(dir);
    w.hidden(false).git_ignore(true).git_global(false).git_exclude(true).require_git(false).max_depth(depth).sort_by_file_path(|a, b| a.cmp(b))
        .filter_entry(|e| !matches!(e.file_name().to_str(), Some(".git" | "node_modules")));
    w
}

fn human(n: u64) -> String {
    match n {
        n if n >= 1 << 20 => format!("{:.1} МБ", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.1} КБ", n as f64 / 1024.0),
        n => format!("{n} Б"),
    }
}

fn list(root: &str, args: &Value) -> Result<Value, String> {
    let (base, dir) = inside(root, args["path"].as_str().unwrap_or("."))?;
    if !dir.is_dir() {
        return Err("это файл, а не папка — читай local_read".into());
    }
    let depth = args["depth"].as_u64().unwrap_or(2).clamp(1, 6) as usize;
    let mut lines = Vec::new();
    let mut more = 0usize;
    for entry in walker(&dir, Some(depth)).build().flatten().skip(1) {
        if lines.len() >= LIST_LIMIT {
            more += 1;
            continue;
        }
        let p = entry.path();
        let indent = "  ".repeat(entry.depth().saturating_sub(1));
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            lines.push(format!("{indent}{name}/"));
        } else {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            lines.push(format!("{indent}{name}  ({})", human(size)));
        }
    }
    let count = lines.len();
    let mut text = format!("{}/\n{}", shown(&base, &dir), lines.join("\n"));
    if more > 0 {
        text.push_str(&format!("\n… и ещё {more} — смотри глубже по частям (path, depth)"));
    }
    Ok(json!({ "ok": true, "text": text, "meta": { "count": count } }))
}

fn load_text(p: &Path) -> Result<String, String> {
    let size = fs::metadata(p).map_err(|e| e.to_string())?.len();
    if size > MAX_FILE {
        return Err(format!("файл слишком большой ({})", human(size)));
    }
    let mut bytes = Vec::new();
    fs::File::open(p).and_then(|mut f| f.read_to_end(&mut bytes)).map_err(|e| e.to_string())?;
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return Err("файл не текстовый".into());
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn cut(line: &str) -> String {
    if line.chars().count() > MAX_LINE {
        line.chars().take(MAX_LINE).collect::<String>() + " …"
    } else {
        line.to_string()
    }
}

fn read(root: &str, args: &Value) -> Result<Value, String> {
    let (_, p) = inside(root, args["path"].as_str().unwrap_or(""))?;
    if p.is_dir() {
        return Err("это папка — смотри local_list".into());
    }
    if secret(&p) {
        return Err("секретный файл (ключи, .env) — модель его не читает".into());
    }
    let text = load_text(&p)?;
    let all: Vec<&str> = text.lines().collect();
    let offset = args["offset"].as_u64().unwrap_or(1).max(1) as usize;
    let limit = args["limit"].as_u64().unwrap_or(400).clamp(1, 2000) as usize;
    let part: Vec<String> = all.iter().enumerate().skip(offset - 1).take(limit)
        .map(|(i, l)| format!("{:>5}→{}", i + 1, cut(l.trim_end_matches('\r')))).collect();
    let mut out = part.join("\n");
    let end = offset - 1 + part.len();
    if end < all.len() {
        out.push_str(&format!("\n… строки {}–{} из {}; дальше — offset {}", offset, end, all.len(), end + 1));
    }
    Ok(json!({ "ok": true, "text": out, "meta": { "lines": part.len(), "total": all.len() } }))
}

fn search(root: &str, args: &Value) -> Result<Value, String> {
    let (base, dir) = inside(root, args["path"].as_str().unwrap_or("."))?;
    let re = RegexBuilder::new(args["pattern"].as_str().unwrap_or(""))
        .case_insensitive(args["ignore_case"].as_bool().unwrap_or(false))
        .size_limit(1 << 22)
        .build()
        .map_err(|e| format!("шаблон не разобрать: {e}"))?;
    let context = args["context"].as_u64().unwrap_or(0).min(3) as usize;
    let limit = args["limit"].as_u64().unwrap_or(100).clamp(1, 400) as usize;
    let mut walk = walker(&dir, None);
    if let Some(glob) = args["glob"].as_str().filter(|g| !g.is_empty()) {
        let mut ob = OverrideBuilder::new(&dir);
        ob.add(glob).map_err(|e| format!("glob не разобрать: {e}"))?;
        walk.overrides(ob.build().map_err(|e| e.to_string())?);
    }
    let started = Instant::now();
    let (mut out, mut count, mut files) = (Vec::new(), 0usize, 0usize);
    'files: for entry in walk.build().flatten() {
        if started.elapsed() > Duration::from_secs(20) {
            out.push("… поиск прерван: дольше 20 секунд, сузь path или glob".into());
            break;
        }
        let p = entry.path();
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) || secret(p)
            || entry.metadata().map(|m| m.len() > MAX_SEARCH_FILE).unwrap_or(true) {
            continue;
        }
        let Ok(text) = load_text(p) else { continue };
        let lines: Vec<&str> = text.lines().collect();
        let mut hit = false;
        for (i, line) in lines.iter().enumerate() {
            if !re.is_match(line) {
                continue;
            }
            hit = true;
            count += 1;
            let rel = shown(&base, p);
            for j in i.saturating_sub(context)..i {
                out.push(format!("{rel}-{}- {}", j + 1, cut(lines[j].trim_end_matches('\r'))));
            }
            out.push(format!("{rel}:{}: {}", i + 1, cut(line.trim_end_matches('\r'))));
            for j in (i + 1)..(i + 1 + context).min(lines.len()) {
                out.push(format!("{rel}-{}- {}", j + 1, cut(lines[j].trim_end_matches('\r'))));
            }
            if count >= limit {
                out.push(format!("… остановился на {limit} совпадениях"));
                break 'files;
            }
        }
        files += hit as usize;
    }
    let text = if count == 0 { "Совпадений нет.".to_string() } else { out.join("\n") };
    Ok(json!({ "ok": true, "text": text, "meta": { "count": count, "files": files } }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("fzcode-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(d.join("src")).unwrap();
        fs::write(d.join("src/a.php"), "<?php\necho 'привет';\n// TODO: поправить\n").unwrap();
        fs::write(d.join(".env"), "SECRET=1").unwrap();
        fs::write(d.join(".gitignore"), "dist/\n").unwrap();
        fs::create_dir_all(d.join("dist")).unwrap();
        fs::write(d.join("dist/x.js"), "TODO").unwrap();
        d
    }

    #[test]
    fn paths_stay_inside_the_folder() {
        let d = tmp();
        let root = d.to_string_lossy().to_string();
        assert!(inside(&root, "src/a.php").is_ok());
        assert!(inside(&root, "../").is_err());
        assert!(inside(&root, "src/../../x").is_err());
        assert!(inside(&root, "C:/Windows/win.ini").is_err());
        assert!(inside(&root, "/etc/passwd").is_err());
        assert!(read(&root, &json!({"path": ".env"})).is_err());
    }

    #[test]
    fn list_read_and_search_work_and_respect_gitignore() {
        let d = tmp();
        let root = d.to_string_lossy().to_string();
        let l = list(&root, &json!({"path": ".", "depth": 3})).unwrap();
        let t = l["text"].as_str().unwrap();
        assert!(t.contains("a.php") && !t.contains("x.js"), "{t}");
        let r = read(&root, &json!({"path": "src/a.php", "offset": 2, "limit": 1})).unwrap();
        assert!(r["text"].as_str().unwrap().starts_with("    2→echo 'привет';"));
        let s = search(&root, &json!({"pattern": "todo", "ignore_case": true})).unwrap();
        assert_eq!(s["meta"]["count"], 1, "{}", s["text"]);
        assert!(s["text"].as_str().unwrap().contains("src/a.php:3:"));
    }
}
