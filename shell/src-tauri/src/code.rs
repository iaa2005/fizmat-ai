// Fizmat Code на компьютере: привязка к аккаунту, папки, которые человек выбрал
// в окне программы, и исполнение вызовов модели (дерево, чтение, поиск, правка,
// команды оболочки — shell.rs). Разрешения спрашивает чат на сайте.
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
            .send_json(json!({ "version": VERSION, "platform": std::env::consts::OS, "shell": crate::shell::detect().name, "roots": roots }));
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
                        let id = call["id"].as_i64().unwrap_or(0);
                        // Команда может идти минутами — своим потоком, чтобы чтение и правка не ждали её.
                        if call["tool"].as_str() == Some("bash") {
                            let (app, token) = (app.clone(), token.clone());
                            std::thread::spawn(move || {
                                let code = app.state::<Code>();
                                let answer = execute(&app, &code, &call, &|| stopped(&token, id));
                                post(&token, id, answer);
                            });
                            continue;
                        }
                        let answer = execute(&app, &code, &call, &|| false);
                        post(&token, id, answer);
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

fn post(token: &str, id: i64, answer: Value) {
    let _ = ureq::post(&format!("{}/api/code/calls/{id}", site()))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/json")
        .timeout(Duration::from_secs(30))
        .send_json(answer);
}

/// Ход остановили («Стоп» в чате) или он перестал ждать — команду пора убить.
fn stopped(token: &str, id: i64) -> bool {
    ureq::get(&format!("{}/api/code/calls/{id}", site()))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/json")
        .timeout(Duration::from_secs(5))
        .call()
        .ok()
        .and_then(|r| r.into_json::<Value>().ok())
        .map(|v| v["stop"].as_bool().unwrap_or(false))
        .unwrap_or(false)
}

// ── Исполнение ───────────────────────────────────────────────────────────
//
// Инструменты и их параметры — как у kimi-code (MIT, Moonshot AI): Read, Write,
// Edit, Glob, Grep; ответы модели — по-английски. Плюс List — дерево папки,
// пока нет оболочки (там kimi пользуется `ls`).

const READ_MAX_CHARS: usize = 100_000;
const MAX_CONTEXT: usize = 20;

fn fail(msg: impl Into<String>) -> Value {
    json!({ "ok": false, "error": msg.into() })
}

fn execute(app: &AppHandle, code: &Code, call: &Value, stop: &dyn Fn() -> bool) -> Value {
    let root = call["root"].as_str().unwrap_or("");
    // Корень — только из своих папок: сайт не может назвать чужую.
    if !code.cfg.lock().unwrap().roots.iter().any(|r| r.path == root) {
        return fail("this folder is not open in the Fizmat app on the computer");
    }
    let args = &call["args"];
    let result = match call["tool"].as_str().unwrap_or("") {
        "list" => list(root, args),
        "read" => read(root, args),
        "glob" => glob(root, args),
        "grep" | "search" => grep(root, args),
        "edit" => edit(app, root, args),
        "write" => write(app, root, args),
        "bash" => bash(root, args, stop),
        other => Err(format!("unknown tool: {other}")),
    };
    result.unwrap_or_else(fail)
}

/// Путь внутри корня: без абсолютных путей и «..», настоящий путь — под корнем.
fn inside(root: &str, rel: &str) -> Result<(PathBuf, PathBuf), String> {
    let base = fs::canonicalize(root).map_err(|_| "the folder does not exist on the computer".to_string())?;
    let rel = rel.trim();
    let rel_path = Path::new(if rel.is_empty() { "." } else { rel });
    if rel_path.is_absolute() || rel.contains(':') || rel_path.components().any(|c| matches!(c, Component::ParentDir | Component::Prefix(_) | Component::RootDir)) {
        return Err("the path must be relative to the folder, without «..»".into());
    }
    let full = fs::canonicalize(base.join(rel_path)).map_err(|_| format!("no such file or directory: {rel}"))?;
    if !full.starts_with(&base) {
        return Err("the path leads outside the folder".into());
    }
    Ok((base, full))
}

fn shown(base: &Path, p: &Path) -> String {
    let rel = p.strip_prefix(base).unwrap_or(p).to_string_lossy().replace('\\', "/");
    if rel.is_empty() { ".".into() } else { rel }
}

/// Секреты модель не читает и не меняет.
fn secret(p: &Path) -> bool {
    let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    name.starts_with(".env") || name.starts_with("id_rsa") || name.starts_with("id_ed25519") || name.starts_with("id_ecdsa")
        || [".pem", ".key", ".pfx", ".p12", ".keystore", ".jks"].iter().any(|ext| name.ends_with(ext))
}

/// Обход с учётом .gitignore; .git — всегда мимо, node_modules — пока не просят включить игнорируемое.
fn walker(dir: &Path, depth: Option<usize>, include_ignored: bool) -> WalkBuilder {
    let mut w = WalkBuilder::new(dir);
    w.hidden(false).git_ignore(!include_ignored).ignore(!include_ignored).git_exclude(!include_ignored).git_global(false)
        .parents(!include_ignored).require_git(false).max_depth(depth).sort_by_file_path(|a, b| a.cmp(b))
        .filter_entry(move |e| match e.file_name().to_str() {
            Some(".git" | ".hg" | ".svn") => false,
            Some("node_modules") => include_ignored,
            _ => true,
        });
    w
}

fn human(n: u64) -> String {
    match n {
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.1} KB", n as f64 / 1024.0),
        n => format!("{n} B"),
    }
}

fn system(text: impl AsRef<str>) -> String {
    format!("<system>{}</system>", text.as_ref())
}

fn list(root: &str, args: &Value) -> Result<Value, String> {
    let (base, dir) = inside(root, args["path"].as_str().unwrap_or("."))?;
    if !dir.is_dir() {
        return Err("this is a file, not a directory — use Read".into());
    }
    let depth = args["depth"].as_u64().unwrap_or(2).clamp(1, 6) as usize;
    let mut lines = Vec::new();
    let mut more = 0usize;
    for entry in walker(&dir, Some(depth), false).build().flatten().skip(1) {
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
        text.push_str(&format!("\n{}", system(format!("{more} more entries not shown — list a subdirectory (path) or use Glob"))));
    }
    Ok(json!({ "ok": true, "text": text, "meta": { "count": count } }))
}

fn load_text(p: &Path) -> Result<String, String> {
    let size = fs::metadata(p).map_err(|e| e.to_string())?.len();
    if size > MAX_FILE {
        return Err(format!("the file is too large ({})", human(size)));
    }
    let mut bytes = Vec::new();
    fs::File::open(p).and_then(|mut f| f.read_to_end(&mut bytes)).map_err(|e| e.to_string())?;
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return Err("this is not a text file".into());
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

/// Read: «номер\tстрока», line_offset (отрицательный — с конца), n_lines, max_chars; продолжение — в <system>.
fn read(root: &str, args: &Value) -> Result<Value, String> {
    let (_, p) = inside(root, args["path"].as_str().unwrap_or(""))?;
    if p.is_dir() {
        return Err("this is a directory — use List or Glob".into());
    }
    if secret(&p) {
        return Err("sensitive file (.env, keys) — not readable".into());
    }
    let text = load_text(&p)?;
    let all: Vec<&str> = text.lines().collect();
    let total = all.len();
    if total == 0 {
        return Ok(json!({ "ok": true, "text": system("The file is empty."), "meta": { "lines": 0, "total": 0 } }));
    }
    let offset = args["line_offset"].as_i64().or_else(|| args["offset"].as_i64()).unwrap_or(1);
    let start = if offset < 0 { total.saturating_sub(offset.unsigned_abs() as usize) } else { (offset.max(1) - 1) as usize };
    if start >= total {
        return Err(format!("line_offset {offset} is past the end: the file has {total} lines"));
    }
    let wanted = args["n_lines"].as_u64().or_else(|| args["limit"].as_u64()).map(|n| n.max(1) as usize).unwrap_or(total - start);
    let stop = (start + wanted).min(total);
    let max = args["max_chars"].as_u64().map(|n| n as usize).unwrap_or(READ_MAX_CHARS).clamp(1000, READ_MAX_CHARS);
    let mut out = String::new();
    let mut end = start;
    for (i, line) in all.iter().enumerate().take(stop).skip(start) {
        let row = format!("{}\t{}\n", i + 1, cut(line.trim_end_matches('\r')));
        if end > start && out.len() + row.len() > max {
            break;
        }
        out.push_str(&row);
        end = i + 1;
    }
    if end < stop {
        out.push_str(&system(format!("Lines {}–{} of {total} shown (max_chars reached). Next Read: line_offset={}, n_lines={}",
            start + 1, end, end + 1, stop - end)));
    } else if stop < total && args["n_lines"].is_null() && args["limit"].is_null() {
        out.push_str(&system(format!("Lines {}–{} of {total}. Next Read: line_offset={}", start + 1, end, end + 1)));
    } else {
        out.push_str(&system(format!("Lines {}–{} of {total}.", start + 1, end)));
    }
    Ok(json!({ "ok": true, "text": out, "meta": { "lines": end - start, "total": total } }))
}

fn mtime(p: &Path) -> std::time::SystemTime {
    fs::metadata(p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH)
}

/// Страница результатов: offset и head_limit (0 — без предела), продолжение — в <system>.
fn page(items: Vec<String>, args: &Value, default_limit: usize, what: &str) -> (String, usize) {
    let total = items.len();
    let offset = args["offset"].as_u64().unwrap_or(0) as usize;
    let limit = args["head_limit"].as_u64().map(|n| n as usize).unwrap_or(default_limit);
    let end = if limit == 0 { total } else { (offset + limit).min(total) };
    let shown_items = if offset < total { items[offset..end].to_vec() } else { Vec::new() };
    let mut out = shown_items.join("\n");
    if end < total {
        out.push_str(&format!("\n{}", system(format!("Showing {what} {}–{end} of {total}. Next: offset={end} with the same arguments", offset + 1))));
    }
    (out, shown_items.len())
}

fn overrides(dir: &Path, pattern: &str) -> Result<ignore::overrides::Override, String> {
    let mut ob = OverrideBuilder::new(dir);
    ob.add(pattern).map_err(|e| format!("bad glob «{pattern}»: {e}"))?;
    ob.build().map_err(|e| e.to_string())
}

/// Glob: файлы по шаблону, свежие сверху.
fn glob(root: &str, args: &Value) -> Result<Value, String> {
    let (base, dir) = inside(root, args["path"].as_str().unwrap_or("."))?;
    let pattern = args["pattern"].as_str().unwrap_or("").trim();
    if pattern.is_empty() {
        return Err("pattern is required".into());
    }
    let mut walk = walker(&dir, None, args["include_ignored"].as_bool().unwrap_or(false));
    walk.overrides(overrides(&dir, pattern)?);
    let started = Instant::now();
    let mut found: Vec<(std::time::SystemTime, String)> = Vec::new();
    for entry in walk.build().flatten() {
        if started.elapsed() > Duration::from_secs(20) {
            break;
        }
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) && !secret(entry.path()) {
            found.push((mtime(entry.path()), shown(&base, entry.path())));
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let total = found.len();
    if total == 0 {
        return Ok(json!({ "ok": true, "text": "No files match.", "meta": { "count": 0 } }));
    }
    let (text, _) = page(found.into_iter().map(|f| f.1).collect(), args, 100, "files");
    Ok(json!({ "ok": true, "text": text, "meta": { "count": total } }))
}

/// Grep: ripgrep-подобный поиск — files_with_matches (по умолчанию), content, count_matches.
fn grep(root: &str, args: &Value) -> Result<Value, String> {
    let (base, target) = inside(root, args["path"].as_str().unwrap_or("."))?;
    let pattern = args["pattern"].as_str().unwrap_or("");
    if pattern.is_empty() {
        return Err("pattern is required".into());
    }
    let multiline = args["multiline"].as_bool().unwrap_or(false);
    let re = RegexBuilder::new(pattern)
        .case_insensitive(args["-i"].as_bool().or_else(|| args["ignore_case"].as_bool()).unwrap_or(false))
        .multi_line(true)
        .dot_matches_new_line(multiline)
        .size_limit(1 << 22)
        .build()
        .map_err(|e| format!("bad regex: {e}"))?;
    let mode = args["output_mode"].as_str().unwrap_or("files_with_matches");
    if !["content", "files_with_matches", "count_matches"].contains(&mode) {
        return Err("output_mode must be content, files_with_matches or count_matches".into());
    }
    let num = |k: &str| args[k].as_u64().map(|n| (n as usize).min(MAX_CONTEXT));
    let ctx = num("-C").or_else(|| num("context"));
    let (before, after) = (ctx.or(num("-B")).unwrap_or(0), ctx.or(num("-A")).unwrap_or(0));
    let numbers = args["-n"].as_bool().unwrap_or(true);

    let mut walk = walker(&target, None, args["include_ignored"].as_bool().unwrap_or(false));
    if let Some(g) = args["glob"].as_str().filter(|g| !g.is_empty()) {
        walk.overrides(overrides(&target, g)?);
    }
    if let Some(t) = args["type"].as_str().filter(|t| !t.is_empty()) {
        let mut tb = ignore::types::TypesBuilder::new();
        tb.add_defaults();
        tb.select(t);
        walk.types(tb.build().map_err(|e| format!("unknown type «{t}»: {e}"))?);
    }

    let started = Instant::now();
    let mut hits: Vec<(std::time::SystemTime, String, Vec<String>, usize)> = Vec::new();
    let (mut matches, mut stopped) = (0usize, false);
    for entry in walk.build().flatten() {
        if started.elapsed() > Duration::from_secs(20) {
            stopped = true;
            break;
        }
        let p = entry.path();
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) || secret(p)
            || entry.metadata().map(|m| m.len() > MAX_SEARCH_FILE).unwrap_or(true) {
            continue;
        }
        let Ok(text) = load_text(p) else { continue };
        let lines: Vec<&str> = text.lines().collect();
        // Строки с совпадениями (многострочное совпадение отмечает все свои строки).
        let starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
        let line_of = |off: usize| starts.partition_point(|&s| s <= off).saturating_sub(1);
        let mut marked = std::collections::BTreeSet::new();
        let mut count = 0usize;
        for m in re.find_iter(&text) {
            count += 1;
            let (a, b) = (line_of(m.start()), line_of(m.end().saturating_sub(1).max(m.start())));
            marked.extend(a..=b);
        }
        if count == 0 {
            continue;
        }
        matches += count;
        let rel = shown(&base, p);
        let mut rows = Vec::new();
        if mode == "content" {
            let mut last: Option<usize> = None;
            let mut keep = std::collections::BTreeSet::new();
            for &i in &marked {
                keep.extend(i.saturating_sub(before)..=(i + after).min(lines.len().saturating_sub(1)));
            }
            for i in keep {
                if last.is_some_and(|l| i > l + 1) {
                    rows.push("--".to_string());
                }
                let sep = if marked.contains(&i) { ':' } else { '-' };
                let line = cut(lines.get(i).copied().unwrap_or("").trim_end_matches('\r'));
                rows.push(if numbers { format!("{rel}{sep}{}{sep}{line}", i + 1) } else { format!("{rel}{sep}{line}") });
                last = Some(i);
            }
        }
        hits.push((mtime(p), rel, rows, count));
    }
    let files = hits.len();
    if files == 0 {
        return Ok(json!({ "ok": true, "text": if stopped { "No matches (search stopped after 20 s — narrow path or glob)." } else { "No matches." },
            "meta": { "count": 0, "files": 0 } }));
    }
    let items: Vec<String> = match mode {
        "content" => hits.into_iter().flat_map(|h| h.2).collect(),
        "count_matches" => hits.iter().map(|h| format!("{}:{}", h.1, h.3)).collect(),
        _ => {
            hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            hits.into_iter().map(|h| h.1).collect()
        }
    };
    let (body, _) = page(items, args, 250, if mode == "content" { "lines" } else { "entries" });
    let mut text = if mode == "count_matches" { format!("Found {matches} matches in {files} files\n{body}") } else { body };
    if stopped {
        text.push_str(&format!("\n{}", system("Search stopped after 20 s — results are partial; narrow path or glob")));
    }
    Ok(json!({ "ok": true, "text": text, "meta": { "count": matches, "files": files } }))
}

// ── Правка (local_edit, local_write) ─────────────────────────────────────

const DIALOG_LINES: usize = 40;
const DIFF_LINES: usize = 400;

/// Путь для записи: файл может ещё не существовать, но его ближайшая
/// существующая папка — внутри корня. Папки по пути создаются при записи.
fn target(root: &str, rel: &str) -> Result<(PathBuf, PathBuf), String> {
    let base = fs::canonicalize(root).map_err(|_| "the folder does not exist on the computer".to_string())?;
    let rel = rel.trim();
    let rel_path = Path::new(rel);
    if rel.is_empty() || rel_path.is_absolute() || rel.contains(':')
        || rel_path.components().any(|c| matches!(c, Component::ParentDir | Component::Prefix(_) | Component::RootDir)) {
        return Err("the path must be relative to the folder, without «..»".into());
    }
    let full = base.join(rel_path);
    if full.exists() {
        let real = fs::canonicalize(&full).map_err(|e| e.to_string())?;
        if !real.starts_with(&base) {
            return Err("the path leads outside the folder".into());
        }
        return Ok((base, real));
    }
    let mut existing = full.parent().map(Path::to_path_buf).unwrap_or_else(|| base.clone());
    while !existing.exists() {
        existing = existing.parent().map(Path::to_path_buf).ok_or("the path leads outside the folder")?;
    }
    if !fs::canonicalize(&existing).map_err(|e| e.to_string())?.starts_with(&base) {
        return Err("the path leads outside the folder".into());
    }
    Ok((base, full))
}

/// Текст файла для правки: только UTF-8, иначе запись испортит файл.
fn load_utf8(p: &Path) -> Result<String, String> {
    let size = fs::metadata(p).map_err(|e| e.to_string())?.len();
    if size > MAX_FILE {
        return Err(format!("the file is too large ({})", human(size)));
    }
    let bytes = fs::read(p).map_err(|e| e.to_string())?;
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return Err("this is not a text file".into());
    }
    String::from_utf8(bytes).map_err(|_| "the file is not UTF-8 — editing would corrupt it".to_string())
}

fn diff(old: &str, new: &str, path: &str) -> (String, usize, usize) {
    let d = similar::TextDiff::from_lines(old, new);
    let (mut added, mut removed) = (0, 0);
    for change in d.iter_all_changes() {
        match change.tag() {
            similar::ChangeTag::Insert => added += 1,
            similar::ChangeTag::Delete => removed += 1,
            similar::ChangeTag::Equal => {}
        }
    }
    let text = d.unified_diff().context_radius(3).header(path, path).to_string();
    let mut lines: Vec<&str> = text.lines().collect();
    let more = lines.len().saturating_sub(DIFF_LINES);
    lines.truncate(DIFF_LINES);
    let mut out = lines.join("\n");
    if more > 0 {
        out.push_str(&format!("\n… {more} more diff lines"));
    }
    (out, added, removed)
}

/// Разрешение — нативным окном программы: страница и сайт его дать не могут.
fn allowed(app: &AppHandle, args: &Value, title: &str, rel: &str, diff: &str) -> bool {
    if !args["confirm"].as_bool().unwrap_or(true) {
        return true;
    }
    let lines: Vec<&str> = diff.lines().filter(|l| !l.starts_with("+++") && !l.starts_with("---")).collect();
    let mut shown = lines.iter().take(DIALOG_LINES).copied().collect::<Vec<_>>().join("\n");
    if lines.len() > DIALOG_LINES {
        shown.push_str(&format!("\n… и ещё {} строк", lines.len() - DIALOG_LINES));
    }
    if let Some(w) = app.webview_windows().values().next() {
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    app.dialog()
        .message(format!("{title}\n{rel}\n\n{shown}"))
        .title("Fizmat Code — разрешить правку?")
        .kind(tauri_plugin_dialog::MessageDialogKind::Warning)
        .buttons(tauri_plugin_dialog::MessageDialogButtons::OkCancelCustom("Разрешить".into(), "Отклонить".into()))
        .blocking_show()
}

/// Журнал правок на компьютере (%APPDATA%/FizmatGuard/code.log): что, где, разрешено ли.
fn journal(root: &str, what: &str, rel: &str, verdict: &str) {
    use std::io::Write;
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let line = format!("{at}\t{what}\t{root}\t{rel}\t{verdict}\n");
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(crate::home().join("code.log")) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Замена строки в файле; концы строк файла (CRLF) сохраняются.
fn replace(text: &str, args: &Value) -> Result<(String, usize), String> {
    let crlf = text.contains("\r\n");
    let norm = |s: &str| if crlf { s.replace("\r\n", "\n").replace('\n', "\r\n") } else { s.to_string() };
    let old = norm(args["old_string"].as_str().unwrap_or(""));
    let new = norm(args["new_string"].as_str().unwrap_or(""));
    if old.is_empty() {
        return Err("old_string is empty".into());
    }
    if old == new {
        return Err("old_string and new_string are identical".into());
    }
    let count = text.matches(old.as_str()).count();
    if count == 0 {
        return Err("old_string not found — Read the file again and copy the text exactly, without the line-number prefix".into());
    }
    let all = args["replace_all"].as_bool().unwrap_or(false);
    if count > 1 && !all {
        return Err(format!("old_string occurs {count} times — add surrounding context to make it unique, or set replace_all"));
    }
    Ok((if all { text.replace(old.as_str(), &new) } else { text.replacen(old.as_str(), &new, 1) }, count))
}

fn edit(app: &AppHandle, root: &str, args: &Value) -> Result<Value, String> {
    let rel = args["path"].as_str().unwrap_or("");
    let (_, p) = inside(root, rel)?;
    if p.is_dir() {
        return Err("this is a directory, not a file".into());
    }
    if secret(&p) {
        return Err("sensitive file (.env, keys) — not editable".into());
    }
    let text = load_utf8(&p)?;
    let (changed, count) = replace(&text, args)?;
    let (d, added, removed) = diff(&text, &changed, rel);
    if !allowed(app, args, "Модель хочет изменить файл", rel, &d) {
        journal(root, "edit", rel, "отклонено");
        return Err("the person declined this edit in the app window".into());
    }
    fs::write(&p, changed.as_bytes()).map_err(|e| format!("cannot write: {e}"))?;
    journal(root, "edit", rel, "сделано");
    let many = if count > 1 { format!(", {count} replacements") } else { String::new() };
    Ok(json!({ "ok": true, "text": format!("Edited {rel}: +{added} −{removed}{many}"),
        "meta": { "diff": d, "added": added, "removed": removed } }))
}

/// Команда в папке (или её подпапке `cwd`); что выполнялось и чем кончилось — в журнал.
fn bash(root: &str, args: &Value, stop: &dyn Fn() -> bool) -> Result<Value, String> {
    let (_, dir) = inside(root, args["cwd"].as_str().unwrap_or("."))?;
    if !dir.is_dir() {
        return Err("cwd is not a directory".into());
    }
    let command = args["command"].as_str().unwrap_or("").trim();
    if command.is_empty() {
        return Err("command is empty".into());
    }
    let r = crate::shell::run(&dir, command, args["timeout"].as_u64().unwrap_or(60), stop)?;
    journal(root, "bash", &command.replace(['\n', '\t'], " "), &format!("exit {}", r["meta"]["exit"]));
    Ok(r)
}

fn write(app: &AppHandle, root: &str, args: &Value) -> Result<Value, String> {
    let rel = args["path"].as_str().unwrap_or("");
    let (_, p) = target(root, rel)?;
    if p.is_dir() {
        return Err("this is a directory, not a file".into());
    }
    if secret(&p) {
        return Err("sensitive file (.env, keys) — not writable".into());
    }
    let content = args["content"].as_str().unwrap_or("");
    let exists = p.exists();
    if exists && args["mode"].as_str() != Some("append") && !args["overwrite"].as_bool().unwrap_or(false) {
        return Err("the file exists — Read it before overwriting, or use Edit".into());
    }
    let append = args["mode"].as_str() == Some("append");
    let before = if exists { load_utf8(&p)? } else { String::new() };
    let after = if append { format!("{before}{content}") } else { content.to_string() };
    let (d, added, removed) = diff(&before, &after, rel);
    let title = match (exists, append) {
        (true, true) => "Модель хочет дописать в файл",
        (true, false) => "Модель хочет переписать файл",
        _ => "Модель хочет создать файл",
    };
    if !allowed(app, args, title, rel, &d) {
        journal(root, "write", rel, "отклонено");
        return Err("the person declined this write in the app window".into());
    }
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create the directory: {e}"))?;
    }
    fs::write(&p, after.as_bytes()).map_err(|e| format!("cannot write: {e}"))?;
    journal(root, "write", rel, "сделано");
    let verb = match (exists, append) { (true, true) => "Appended to", (true, false) => "Overwrote", _ => "Created" };
    Ok(json!({ "ok": true, "text": format!("{verb} {rel}: +{added} −{removed}"),
        "meta": { "diff": d, "added": added, "removed": removed, "created": !exists } }))
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
    fn list_read_glob_and_grep_work_and_respect_gitignore() {
        let d = tmp();
        let root = d.to_string_lossy().to_string();
        let l = list(&root, &json!({"path": ".", "depth": 3})).unwrap();
        let t = l["text"].as_str().unwrap();
        assert!(t.contains("a.php") && !t.contains("x.js"), "{t}");
        let r = read(&root, &json!({"path": "src/a.php", "line_offset": 2, "n_lines": 1})).unwrap();
        assert!(r["text"].as_str().unwrap().starts_with("2\techo 'привет';"), "{}", r["text"]);
        let tail = read(&root, &json!({"path": "src/a.php", "line_offset": -1})).unwrap();
        assert!(tail["text"].as_str().unwrap().starts_with("3\t// TODO"), "{}", tail["text"]);
        let g = glob(&root, &json!({"pattern": "*.php"})).unwrap();
        assert_eq!(g["text"].as_str().unwrap(), "src/a.php");
        let files = grep(&root, &json!({"pattern": "todo", "-i": true})).unwrap();
        assert_eq!(files["text"].as_str().unwrap(), "src/a.php", "files_with_matches по умолчанию, dist/ в .gitignore");
        let content = grep(&root, &json!({"pattern": "TODO", "output_mode": "content", "-B": 1})).unwrap();
        assert_eq!(content["text"].as_str().unwrap(), "src/a.php-2-echo 'привет';\nsrc/a.php:3:// TODO: поправить");
        let counts = grep(&root, &json!({"pattern": "TODO", "output_mode": "count_matches", "include_ignored": true})).unwrap();
        assert!(counts["text"].as_str().unwrap().starts_with("Found 2 matches in 2 files"), "{}", counts["text"]);
    }

    #[test]
    fn edits_keep_line_endings_and_stay_inside() {
        let d = tmp();
        let root = d.to_string_lossy().to_string();
        let text = "a\r\nb\r\nb\r\n";
        assert!(replace(text, &json!({"old_string": "b", "new_string": "c"})).is_err(), "два совпадения");
        let (out, n) = replace(text, &json!({"old_string": "a\nb", "new_string": "x\ny"})).unwrap();
        assert_eq!((out.as_str(), n), ("x\r\ny\r\nb\r\n", 1));
        assert!(target(&root, "new/deep/f.txt").is_ok());
        assert!(target(&root, "../x.txt").is_err());
        let (diff_text, added, removed) = diff("a\nb\n", "a\nc\n", "f");
        assert_eq!((added, removed), (1, 1));
        assert!(diff_text.contains("+c"));
    }
}
