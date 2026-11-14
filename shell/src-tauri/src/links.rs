// Новые окна и скачивания встроенного браузера (WebView2).
//
// Tauri по умолчанию молча отбрасывает и то и другое: ссылка с target="_blank" (пакеты хранилища,
// «Открыть» у файла, внешние PDF) не открывалась никак, а «Скачать» ничего не сохраняло.
// Теперь:
//   • страница fizmat.ai «в новом окне» (пакет, файл) — в том же окне программы;
//   • чужой сайт — обычный браузер человека (там и PDF, и всё остальное);
//   • просмотр файла (PDF или картинка по временной ссылке хранилища, файл чата) — отдельным окном
//     с обычной рамкой Windows: у просмотрщика PDF нет кнопок окна и «Назад», основное окно
//     уходить туда не должно — из него было бы не выбраться;
//   • скачивание — во временную папку, потом окно Windows «Сохранить как», файл — куда выбрали.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use tauri::webview::{DownloadEvent, NewWindowResponse};
use tauri::{AppHandle, Manager, Url, Webview, WebviewUrl, WebviewWindowBuilder, Wry};
use tauri_plugin_dialog::DialogExt;

/// Страница нашего сайта: её окна получают права программы (capabilities/remote.json).
pub fn is_site(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some_and(|h| h == "fizmat.ai" || h.ends_with(".fizmat.ai") || h == "new.fizmat.space")
}

static VIEWS: AtomicU32 = AtomicU32::new(1);

/// Файл, который откроется просмотром, а не скачается: временная ссылка хранилища (S3) или файл
/// чата/проекта на сайте — без признака скачивания.
pub fn is_file(url: &Url) -> bool {
    static ROUTE: OnceLock<regex::Regex> = OnceLock::new();
    let query = url.query().unwrap_or("").to_lowercase();
    if query.contains("response-content-disposition=attachment") || query.contains("download=1") {
        return false;
    }
    let route = ROUTE.get_or_init(|| regex::Regex::new(r"^/(c|projects)/[^/]+/files/[^/]+/?$").unwrap());
    url.host_str() == Some("s3.regru.cloud") || (is_site(url) && route.is_match(url.path()))
}

/// Основное окно хочет уйти на файл — вместо этого окно просмотра; true — пусть идёт.
pub fn navigation(app: &AppHandle, url: &Url) -> bool {
    if is_file(url) {
        view(app, url.clone());
        return false;
    }
    true
}

/// Окно просмотра файла: с рамкой Windows (закрыть крестиком), ссылки из него — в браузер.
pub fn view(app: &AppHandle, url: Url) {
    let app = app.clone();
    std::thread::spawn(move || {
        let label = format!("view-{}", VIEWS.fetch_add(1, Ordering::SeqCst));
        let _ = WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(url))
            .title("Fizmat — просмотр файла")
            .inner_size(1100.0, 820.0)
            .on_new_window(|u, _| {
                external(u.as_str());
                NewWindowResponse::Deny
            })
            .on_download(|webview, event| download(&webview, event))
            .build();
    });
}

/// `from` — окно, где нажали ссылку: страница сайта открывается в нём же, файл — окном просмотра.
pub fn new_window(app: &AppHandle, from: &str, url: Url) -> NewWindowResponse<Wry> {
    if is_file(&url) {
        view(app, url);
    } else if is_site(&url) {
        // Переход — не из обработчика события WebView2, а отдельным потоком.
        let (app, from) = (app.clone(), from.to_string());
        std::thread::spawn(move || {
            if let Some(w) = app.get_webview_window(&from) {
                let _ = w.navigate(url);
            }
        });
    } else if matches!(url.scheme(), "http" | "https" | "mailto") {
        external(url.as_str());
    }
    NewWindowResponse::Deny
}

/// Обычный браузер человека (или почта для mailto:).
pub fn external(url: &str) {
    let mut cmd = std::process::Command::new("rundll32");
    cmd.args(["url.dll,FileProtocolHandler", url]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let _ = cmd.spawn();
}

pub fn download(webview: &Webview, event: DownloadEvent<'_>) -> bool {
    match event {
        DownloadEvent::Requested { destination, .. } => {
            let name = destination.file_name().map(|n| n.to_owned()).unwrap_or_else(|| "файл".into());
            let dir = std::env::temp_dir().join("fizmat-downloads").join(uuid::Uuid::new_v4().simple().to_string());
            if std::fs::create_dir_all(&dir).is_ok() {
                *destination = dir.join(name);
            }
            true
        }
        DownloadEvent::Finished { path, success, .. } => {
            if success {
                if let Some(p) = path {
                    save_as(webview.app_handle(), p);
                }
            }
            true
        }
        _ => true,
    }
}

/// «Сохранить как»: папка — «Загрузки», имя — как у сайта. Отмена — временный файл удаляется.
fn save_as(app: &AppHandle, temp: PathBuf) {
    let name = temp.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "файл".into());
    let mut dialog = app.dialog().file().set_title("Сохранить файл").set_file_name(&name);
    if let Some(dir) = downloads() {
        dialog = dialog.set_directory(dir);
    }
    if let Some(ext) = Path::new(&name).extension().map(|e| e.to_string_lossy().to_string()) {
        dialog = dialog.add_filter(ext.to_uppercase(), &[ext.as_str()]);
    }
    dialog.save_file(move |chosen| {
        match chosen.and_then(|f| f.into_path().ok()) {
            Some(target) => {
                if std::fs::rename(&temp, &target).is_err() {
                    let _ = std::fs::copy(&temp, &target);
                    let _ = std::fs::remove_file(&temp);
                }
            }
            None => {
                let _ = std::fs::remove_file(&temp);
            }
        }
        if let Some(dir) = temp.parent() {
            let _ = std::fs::remove_dir(dir);
        }
    });
}

fn downloads() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE")?;
    let dir = PathBuf::from(home).join("Downloads");
    dir.is_dir().then_some(dir)
}
