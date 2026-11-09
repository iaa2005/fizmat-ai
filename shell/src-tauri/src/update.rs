// Обновление программы само собой: человек ничего не переустанавливает.
//
// При запуске, до окна fizmat.ai, программа спрашивает GitHub (latest.json выпуска)
// не дольше нескольких секунд. Есть новее — маленькое окно «Обновляю Fizmat…»,
// установщик ставится тихо (NSIS passive) и программа запускается заново. Пока
// программа живёт в трее, она проверяет раз в шесть часов и ставит новую версию,
// только когда окно спрятано и не идёт ни одной команды модели.
//
// Установщик подписан ключом выпуска (minisign); открытая половина — в tauri.conf.json,
// чужой установщик программа не поставит.

use std::time::Duration;

use tauri::{AppHandle, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_updater::UpdaterExt;

/// Заглушка сайта (FIZMAT_SITE) или явный запрет — без обновлений: проверки и сборка для себя.
fn off() -> bool {
    std::env::var_os("FIZMAT_SITE").is_some() || std::env::var_os("FIZMAT_NO_UPDATE").is_some() || cfg!(debug_assertions)
}

/// Проверка обновления без установки: FIZMAT_UPDATE_URL — свой latest.json, FIZMAT_UPDATE_DRY —
/// скачать, сверить подпись и записать итог в code.log, ничего не ставя.
fn dry() -> bool {
    std::env::var_os("FIZMAT_UPDATE_DRY").is_some()
}

async fn find(app: &AppHandle, wait: Duration) -> Option<tauri_plugin_updater::Update> {
    let url = std::env::var("FIZMAT_UPDATE_URL").ok().and_then(|u| u.parse().ok());
    if off() && url.is_none() {
        return None;
    }
    let mut builder = app.updater_builder().timeout(wait);
    if let Some(url) = url {
        builder = builder.endpoints(vec![url]).ok()?;
    }
    let updater = match builder.build() {
        Ok(u) => u,
        Err(e) => {
            crate::code::note("update", &format!("no updater: {e}"));
            return None;
        }
    };
    match tokio::time::timeout(wait, updater.check()).await {
        Ok(Ok(found)) => found,
        Ok(Err(e)) => {
            crate::code::note("update", &format!("check failed: {e}"));
            None
        }
        Err(_) => None,
    }
}

async fn install(app: &AppHandle, update: tauri_plugin_updater::Update) -> bool {
    crate::code::note("update", &format!("{} → {}", update.current_version, update.version));
    if dry() {
        let got = update.download(|_, _| {}, || {}).await;
        crate::code::note("update", &match got {
            Ok(bytes) => format!("dry run: downloaded {} bytes, signature ok", bytes.len()),
            Err(e) => format!("dry run: {e}"),
        });
        return false;
    }
    let ready = app.clone();
    match update.download_and_install(|_, _| {}, move || crate::before_update(&ready)).await {
        Ok(()) => {
            app.restart();
        }
        Err(e) => {
            crate::code::note("update", &format!("not installed: {e}"));
            false
        }
    }
}

/// При запуске: новая версия есть — поставить её и перезапуститься (тогда `open` не зовётся).
pub fn at_start(app: AppHandle, open: impl FnOnce(&AppHandle) + Send + 'static) {
    tauri::async_runtime::spawn(async move {
        if let Some(update) = find(&app, Duration::from_secs(5)).await {
            let notice = WebviewWindowBuilder::new(&app, "update", WebviewUrl::App("update.html".into()))
                .title("Fizmat")
                .inner_size(360.0, 160.0)
                .resizable(false)
                .center()
                .build();
            install(&app, update).await;
            // Не встала — работаем на старой: сначала окно, потом закрыть заглушку (иначе окон
            // ноль и программа выйдет).
            open(&app);
            if let Ok(w) = notice {
                let _ = w.close();
            }
            return;
        }
        open(&app);
    });
}

/// В трее: раз в шесть часов; ставить, только когда окно спрятано и команды не идут.
pub fn in_background(app: AppHandle, idle: impl Fn(&AppHandle) -> bool + Send + 'static) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
            if let Some(update) = find(&app, Duration::from_secs(30)).await {
                while !idle(&app) {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
                install(&app, update).await;
            }
        }
    });
}
