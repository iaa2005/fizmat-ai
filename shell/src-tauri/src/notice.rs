// Уведомления сайта — уведомлениями Windows.
//
// Браузерный push в окне программы не работает (WebView2 отказывает в разрешении), поэтому
// сайт кладёт для этого компьютера новости (сообщения и звонки «Общения», «модель ждёт
// разрешения», готовые задачи) в ответ на опрос /api/code/calls — поле news. Здесь их
// показываем; нажали — окно Fizmat открывается на нужной странице. Человек смотрит на этот
// же чат в окне программы — не мешаем.

use serde_json::Value;
use tauri::{AppHandle, Manager};

pub fn show(app: &AppHandle, news: &[Value]) {
    let main = app.get_webview_window("main");
    let focused = main.as_ref().and_then(|w| w.is_focused().ok()).unwrap_or(false);
    let here = main.as_ref().and_then(|w| w.url().ok()).map(|u| u.to_string()).unwrap_or_default();
    for n in news {
        let (title, body, url) = (n["title"].as_str().unwrap_or("Fizmat"), n["body"].as_str().unwrap_or(""), n["url"].as_str().unwrap_or(""));
        if focused && seen(&here, url) {
            continue;
        }
        #[cfg(windows)]
        toast(app, title, body, url);
    }
}

/// Открыта та же страница (или «Общение» с этим чатом справа): последний кусок адреса уже в адресе окна.
fn seen(here: &str, url: &str) -> bool {
    let tail = url.split(['?', '#']).next().unwrap_or("").trim_end_matches('/').rsplit('/').next().unwrap_or("");
    !tail.is_empty() && here.contains(tail)
}

#[cfg(windows)]
fn toast(app: &AppHandle, title: &str, body: &str, url: &str) {
    use tauri_winrt_notification::{Duration, Toast};
    let id = app.config().identifier.clone();
    let (app, url) = (app.clone(), url.to_string());
    let _ = Toast::new(&id)
        .title(title)
        .text1(body)
        .duration(Duration::Short)
        .on_activated(move |_| {
            open(&app, &url);
            Ok(())
        })
        .show();
}

/// Нажали на уведомление: окно — вперёд, в нём — адрес уведомления (только страницы сайта).
fn open(app: &AppHandle, url: &str) {
    if let Some(w) = app.get_webview_window("main") {
        if url.starts_with(&crate::code::site()) {
            if let Ok(u) = url.parse() {
                let _ = w.navigate(u);
            }
        }
    }
    crate::tray::show_main(app);
}
