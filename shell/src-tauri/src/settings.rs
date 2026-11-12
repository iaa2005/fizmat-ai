// Окно настроек программы (dist/settings.html): «Управление компьютером» и «Не гасить экран и не
// блокировать». Включить управление — только после подтверждения в родном окне Windows: ни страница
// fizmat.ai, ни модель (окна Fizmat ей недоступны) этого сделать не могут. Выключить — сразу.

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use crate::code;

pub fn open(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title("Настройки Fizmat")
        .inner_size(480.0, 430.0)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .center()
        .build();
}

fn state(app: &AppHandle) -> Value {
    let c = app.state::<code::Code>();
    json!({ "computer": code::computer_on(&c), "awake": code::awake_on(&c), "paired": code::paired(&c), "name": code::computer_name() })
}

fn changed(app: &AppHandle) {
    let _ = app.emit_to("settings", "settings-changed", ());
}

#[tauri::command]
pub fn settings_get(app: AppHandle) -> Value {
    state(&app)
}

#[tauri::command]
pub async fn settings_set(key: String, on: bool, app: AppHandle) -> Result<Value, String> {
    match (key.as_str(), on) {
        ("awake", _) => code::set_awake(&app.state::<code::Code>(), on),
        ("computer", false) => {
            code::set_computer(&app.state::<code::Code>(), false);
            #[cfg(windows)]
            crate::veil::finish(&app);
        }
        ("computer", true) => {
            let later = app.clone();
            app.dialog()
                .message("Модель сможет видеть экран этого компьютера, двигать мышь и печатать — в приложениях, которые вы разрешите ей в чате.\n\nБраузеры остаются только для просмотра, терминалы — только для щелчков. Выключить можно в настройках в любой момент.")
                .title("Разрешить управление компьютером?")
                .kind(MessageDialogKind::Warning)
                .buttons(MessageDialogButtons::OkCancelCustom("Разрешить".into(), "Отмена".into()))
                .show(move |yes| {
                    std::thread::spawn(move || {
                        if yes {
                            code::set_computer(&later.state::<code::Code>(), true);
                        }
                        changed(&later);
                    });
                });
        }
        _ => return Err("нет такой настройки".into()),
    }
    Ok(state(&app))
}
