// Fizmat с защитой личных данных: окно с fizmat.ai, рядом — локальная прослойка
// (PII-Tracer + правила, guard/server.py). Страница зовёт команду guard, программа
// передаёт запрос прослойке с ключом; сама страница ключа не видит.
// В установленной программе прослойка — guard/guard.exe среди ресурсов (PyInstaller),
// модель она скачивает сама при первом запуске; в сборке для себя — server.py из .venv.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

mod code;
mod glass;
mod shell;
mod tray;
mod update;

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

/// Окно fizmat.ai. Крестик прячет его в трей: Code ждёт вызовы модели и с закрытым окном.
fn main_window(app: &AppHandle) {
    let script = format!("window.__FZ_GUARD = {{ ipc: true }};\n{INJECT}\n{HERE}");
    let built = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(START_URL.parse().unwrap()))
        .title("Fizmat")
        .inner_size(1280.0, 860.0)
        .min_inner_size(420.0, 560.0)
        .initialization_script(&script)
        .build();
    let Ok(window) = built else { return };
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
            tray::quick_ask, tray::quick_hide])
        .setup(|app| {
            // Fizmat Code: компьютер уже привязан — сразу спрашивать вызовы модели.
            code::start(app.handle().clone());
            *app.state::<Guard>().child.lock().unwrap() = start_service(app.path().resource_dir().ok());
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
