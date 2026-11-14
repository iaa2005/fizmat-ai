// Fizmat для Windows: окно с fizmat.ai, трей, окружение чатов (папки, команды, Remote Control).
// Прослойка личных данных (guard) с 0.6.5 не запускается и в страницу не встраивается:
// её код ниже оставлен мёртвым до удаления, команда guard отвечает отказом.
#![allow(dead_code)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

mod code;
#[cfg(windows)]
mod computer;
mod glass;
mod links;
mod notice;
mod settings;
mod shell;
#[cfg(windows)]
mod awake;
mod tray;
#[cfg(windows)]
mod ui;
mod update;
#[cfg(windows)]
mod veil;

use serde_json::Value;
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};

const PORT: u16 = 47821;
const START_URL: &str = "https://fizmat.ai/home";
const INJECT: &str = include_str!("../../../guard/inject.js");

/// Страница fizmat.ai в этом окне говорит сайту «я в программе на этом компьютере» (Remote
/// Control у чатов Code): билет программы → POST /code/here. Раз на сессию сайта — ключом
/// служит её csrf-токен (он меняется со входом и выходом).
const HERE: &str = r#"
(() => {
    if (window.top !== window) return;
    const go = async () => {
        const invoke = window.__TAURI__?.core?.invoke;
        const csrf = document.querySelector('meta[name=csrf-token]')?.content;
        if (!invoke || !csrf || sessionStorage.getItem('fz-here') === csrf) return;
        try {
            const { ticket } = await invoke('code_ticket');
            const r = await fetch('/code/here', { method: 'POST', credentials: 'same-origin', body: JSON.stringify({ ticket }),
                headers: { 'Content-Type': 'application/json', Accept: 'application/json', 'X-CSRF-TOKEN': csrf } });
            if (!r.ok) return;
            sessionStorage.setItem('fz-here', csrf);
            if ((await r.json()).changed) window.ChatSidebar?.refresh?.();
        } catch (e) { /* не привязан или старая программа */ }
    };
    document.readyState === 'loading' ? addEventListener('DOMContentLoaded', go) : go();
})();
"#;

/// Заголовок окна — под тему сайта: страница говорит программе, тёмная она или светлая.
const THEME: &str = r#"
(() => {
    if (window.top !== window) return;
    const tell = () => {
        const invoke = window.__TAURI__?.core?.invoke;
        if (!invoke) return;
        const dark = (document.documentElement.dataset.theme || (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light')) === 'dark';
        invoke('window_theme', { dark }).catch(() => {});
    };
    addEventListener('themechange', tell);
    matchMedia('(prefers-color-scheme: dark)').addEventListener('change', tell);
    document.readyState === 'loading' ? addEventListener('DOMContentLoaded', tell) : tell();
})();
"#;

/// Тёмный или светлый заголовок окна; на Windows 11 — ещё и точный цвет фона сайта.
#[tauri::command]
fn window_theme(dark: bool, app: AppHandle) {
    let Some(w) = app.get_webview_window("main") else { return };
    let _ = w.set_theme(Some(if dark { tauri::Theme::Dark } else { tauri::Theme::Light }));
    #[cfg(windows)]
    if let Ok(hwnd) = w.hwnd() {
        use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
        // COLORREF 0x00BBGGRR: #181818 и #ffffff — фон страниц; текст заголовка — под него.
        let (caption, text): (u32, u32) = if dark { (0x0018_1818, 0x00EF_F4F5) } else { (0x00FF_FFFF, 0x0000_0000) };
        unsafe {
            DwmSetWindowAttribute(hwnd.0 as _, 35, &caption as *const u32 as _, 4);
            DwmSetWindowAttribute(hwnd.0 as _, 34, &caption as *const u32 as _, 4);
            DwmSetWindowAttribute(hwnd.0 as _, 36, &text as *const u32 as _, 4);
        }
    }
}

struct Guard {
    token: String,
    child: Mutex<Option<Child>>,
}

pub(crate) fn home() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("FizmatGuard")
}

/// Ключ прослойки: общий с server.py через файл, создаётся один раз.
fn token() -> String {
    let path = home().join("token");
    if let Ok(t) = fs::read_to_string(&path) {
        if !t.trim().is_empty() {
            return t.trim().to_string();
        }
    }
    let t = uuid::Uuid::new_v4().simple().to_string() + &uuid::Uuid::new_v4().simple().to_string();
    let _ = fs::create_dir_all(home());
    let _ = fs::write(&path, &t);
    t
}

/// Папка guard/: из FIZMAT_GUARD_DIR, рядом с программой или в исходниках (сборка для себя).
fn guard_dir() -> Option<PathBuf> {
    let has = |p: &Path| p.join("server.py").exists();
    if let Some(dir) = std::env::var_os("FIZMAT_GUARD_DIR").map(PathBuf::from) {
        if has(&dir) {
            return Some(dir);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        for up in exe.ancestors() {
            if has(&up.join("guard")) {
                return Some(up.join("guard"));
            }
        }
    }
    let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../guard");
    has(&dev).then_some(dev)
}

fn alive() -> bool {
    ureq::get(&format!("http://127.0.0.1:{PORT}/health"))
        .timeout(Duration::from_millis(400))
        .call()
        .is_ok()
}

/// Запуск прослойки без окна консоли. Уже запущенную (вторая копия программы) не трогаем.
fn start_service(resources: Option<PathBuf>) -> Option<Child> {
    if alive() {
        return None;
    }
    let bundled = resources.map(|r| r.join("guard").join("guard.exe")).filter(|p| p.exists());
    let mut cmd = match bundled {
        Some(exe) => {
            let mut c = Command::new(&exe);
            c.current_dir(exe.parent()?);
            c
        }
        None => {
            let dir = guard_dir()?;
            let mut c = Command::new(dir.join("../.venv/Scripts/python.exe"));
            c.arg("server.py").current_dir(&dir);
            c
        }
    };
    let _ = fs::create_dir_all(home());
    let log = fs::File::create(home().join("guard.log")).ok()?;
    cmd.arg(PORT.to_string())
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null()).stdout(log.try_clone().ok()?).stderr(log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.spawn().ok()
}

/// Запрос страницы к прослойке. Только три пути и здоровье — больше страница ничего не может.
#[tauri::command]
fn guard(path: String, body: Option<Value>, state: tauri::State<Guard>) -> Result<Value, String> {
    if !alive() {
        return Err("прослойка личных данных отключена".into());
    }
    let url = format!("http://127.0.0.1:{PORT}{path}");
    let res = match path.as_str() {
        "/health" => ureq::get(&url).timeout(Duration::from_secs(2)).call(),
        "/scan" | "/mask" | "/restore" | "/vault" | "/vault/clear" => ureq::post(&url)
            .set("X-Guard-Token", &state.token)
            .timeout(Duration::from_secs(60))
            .send_json(body.unwrap_or(Value::Null)),
        _ => return Err("нет такого пути".into()),
    };
    match res {
        Ok(r) => r.into_json::<Value>().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(code, _)) => Err(format!("прослойка ответила {code}")),
        Err(_) => Err("прослойка ещё запускается".into()),
    }
}

/// Окно без рамки Windows: кнопки «свернуть / развернуть / закрыть» и шапку, за которую тянут,
/// рисует сама страница (fizmat.ai, partials/app-frame) — по этому флагу.
const FRAMELESS: &str = "window.__FZ_FRAMELESS = true;";

/// Окно страницы fizmat.ai — главное и те, что страница открывает в новом окне (target="_blank"):
/// без рамки, со скриптами программы, новые окна и скачивания — через links.rs.
pub(crate) fn site_window(app: &AppHandle, label: &str, url: tauri::Url) -> Option<tauri::WebviewWindow> {
    let script = format!("{FRAMELESS}\n{HERE}\n{THEME}");
    let (opener, from) = (app.clone(), label.to_string());
    let (w, h) = if label == "main" { (1280.0, 860.0) } else { (1180.0, 820.0) };
    WebviewWindowBuilder::new(app, label, WebviewUrl::External(url))
        .title("Fizmat")
        .inner_size(w, h)
        .min_inner_size(420.0, 560.0)
        .decorations(false)
        .shadow(true)
        .initialization_script(&script)
        .on_new_window(move |url, _| links::new_window(&opener, &from, url))
        .on_navigation({
            let app = app.clone();
            move |url| links::navigation(&app, url)
        })
        .on_download(|webview, event| links::download(&webview, event))
        .build()
        .ok()
}

/// Окно fizmat.ai. Крестик прячет его в трей: Code ждёт вызовы модели и с закрытым окном.
fn main_window(app: &AppHandle) {
    let Some(window) = site_window(app, "main", START_URL.parse().unwrap()) else { return };
    let hidden = window.clone();
    window.on_window_event(move |e| {
        if let WindowEvent::CloseRequested { api, .. } = e {
            api.prevent_close();
            let _ = hidden.hide();
        }
    });
}

/// Перед установкой новой версии: прослойка и команды не должны держать файлы программы.
pub(crate) fn before_update(app: &AppHandle) {
    shell::stop_all();
    if let Some(mut child) = app.state::<Guard>().child.lock().unwrap().take() {
        let _ = child.kill();
    }
}

fn main() {
    let app = tauri::Builder::default()
        .manage(Guard { token: token(), child: Mutex::new(None) })
        .manage(code::Code::load())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![guard, code::code_status, code::code_pair, code::code_add_root, code::code_remove_root, code::code_ticket,
            tray::quick_ask, tray::quick_hide, settings::settings_get, settings::settings_set, window_theme,
            code::code_reveal, code::code_terminal])
        .setup(|app| {
            // Fizmat Code: компьютер уже привязан — сразу спрашивать вызовы модели.
            code::start(app.handle().clone());
            // «Не гасить экран» — как было до перезапуска.
            #[cfg(windows)]
            if code::awake_on(&app.state::<code::Code>()) {
                awake::set(true);
            }
            tray::build(app.handle())?;
            // Сначала — новая версия, если она есть (ставится сама); потом окно fizmat.ai.
            update::at_start(app.handle().clone(), main_window);
            update::in_background(app.handle().clone(), |app| {
                shell::running() == 0 && !app.get_webview_window("main").and_then(|w| w.is_visible().ok()).unwrap_or(false)
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("не запустилась оболочка");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            if let Some(mut child) = handle.state::<Guard>().child.lock().unwrap().take() {
                let _ = child.kill();
            }
        }
    });
}
