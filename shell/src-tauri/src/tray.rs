// Значок в трее, как у Claude для компьютера.
//
// Левая кнопка — строка «Чем помочь?» поверх экрана (окно quick, dist/quick.html): вопрос
// уходит новым чатом fizmat.ai с моделью по умолчанию, окно программы открывается на нём.
// Правая кнопка — меню: открыть Fizmat, подписка и расход (GET /api/code/me раз в минуту), «Настройки…»
// (управление компьютером и «не гасить экран» — окно settings.rs),
// остановить команды модели, выйти. Крестик главного окна прячет его сюда (main.rs).

use std::time::Duration;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use crate::{code, shell};

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Строка быстрого вопроса: без рамки, прозрачная, поверх окон, по центру в верхней трети экрана.
/// Теряет фокус — прячется (как у Claude).
fn quick(app: &AppHandle) {
    let window = match app.get_webview_window("quick") {
        Some(w) => w,
        None => {
            let Ok(w) = WebviewWindowBuilder::new(app, "quick", WebviewUrl::App("quick.html".into()))
                .title("Fizmat")
                .inner_size(680.0, 120.0)
                .decorations(false)
                .transparent(true)
                .shadow(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .resizable(false)
                .visible(false)
                .build()
            else {
                return;
            };
            let hide = w.clone();
            w.on_window_event(move |e| {
                if std::env::var_os("FIZMAT_QUICK").is_some() {
                    code::note("quick", &format!("{e:?}"));
                }
                // Фокус уходит во вложенный WebView2 — окну приходит ложное «потерян фокус»;
                // прятать, только если и чуть позже фокус не у этого окна (щёлкнули мимо).
                if let WindowEvent::Focused(false) = e {
                    let hide = hide.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(150));
                        if !hide.is_focused().unwrap_or(false) {
                            crate::glass::vanish(&hide);
                        }
                    });
                }
            });
            w
        }
    };
    if let (Ok(Some(monitor)), Ok(size)) = (window.current_monitor(), window.outer_size()) {
        let area = monitor.size();
        let at = monitor.position();
        let x = at.x + (area.width as i32 - size.width as i32) / 2;
        let y = at.y + area.height as i32 / 4;
        let _ = window.set_position(PhysicalPosition::new(x, y));
    }
    if std::env::var_os("FIZMAT_QUICK").is_some() {
        code::note("quick", &format!("show at {:?} size {:?}", window.outer_position(), window.outer_size()));
    }
    crate::glass::appear(&window);
}

/// Вопрос из строки: новый чат на fizmat.ai (форма POST /c — сайт сам отправит вопрос
/// моделью по умолчанию) в главном окне.
#[tauri::command]
pub fn quick_ask(text: String, app: AppHandle) -> Result<(), String> {
    let text = text.trim();
    if let Some(q) = app.get_webview_window("quick") {
        crate::glass::vanish(&q);
    }
    if text.is_empty() {
        return Ok(());
    }
    let main = app.get_webview_window("main").ok_or("окно Fizmat ещё не открылось")?;
    let q = serde_json::to_string(text).map_err(|e| e.to_string())?;
    main.eval(&format!(
        r#"(() => {{
            const token = document.querySelector('meta[name=csrf-token]')?.content;
            if (!token) {{ location.href = 'https://fizmat.ai/home'; return; }}
            const f = document.createElement('form');
            f.method = 'POST'; f.action = '/c'; f.style.display = 'none';
            for (const [k, v] of [['_token', token], ['q', {q}]]) {{
                const i = document.createElement('textarea'); i.name = k; i.value = v; f.appendChild(i);
            }}
            document.body.appendChild(f); f.submit();
        }})()"#
    ))
    .map_err(|e| e.to_string())?;
    show_main(&app);
    Ok(())
}

#[tauri::command]
pub fn quick_hide(app: AppHandle) {
    if let Some(q) = app.get_webview_window("quick") {
        crate::glass::vanish(&q);
    }
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Открыть Fizmat", true, None::<&str>)?;
    let plan = MenuItem::with_id(app, "plan", "Компьютер не привязан", false, None::<&str>)?;
    let usage = MenuItem::with_id(app, "usage", "Расход: —", false, None::<&str>)?;
    let stop = MenuItem::with_id(app, "stop", "Остановить команды", false, None::<&str>)?;
    // Управление компьютером и «не гасить экран» — в окне настроек.
    let settings = MenuItem::with_id(app, "settings", "Настройки…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Выйти", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[
        &open,
        &PredefinedMenuItem::separator(app)?,
        &plan,
        &usage,
        &PredefinedMenuItem::separator(app)?,
        &settings,
        &stop,
        &quit,
    ])?;
    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("Fizmat")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, e| match e.id.as_ref() {
            "open" => show_main(app),
            "settings" => crate::settings::open(app),
            "stop" => shell::stop_all(),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                quick(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    let icon = builder.build(app)?;
    // Проверка вида строки без щелчка по трею.
    if std::env::var_os("FIZMAT_QUICK").is_some() {
        let later = app.clone();
        std::thread::spawn(move || {
            // Показать, спрятать и показать снова — второй показ и есть обычный щелчок по трею.
            for (wait, show) in [(4, true), (2, false), (2, true)] {
                std::thread::sleep(Duration::from_secs(wait));
                let again = later.clone();
                let _ = later.run_on_main_thread(move || {
                    if show {
                        quick(&again)
                    } else if let Some(w) = again.get_webview_window("quick") {
                        crate::glass::vanish(&w);
                    }
                });
            }
        });
    }

    // Подсказка и «Остановить команды» — раз в три секунды; подписка и расход — раз в минуту.
    let handle = app.clone();
    std::thread::spawn(move || {
        let mut tick = 0u32;
        loop {
            let n = shell::running();
            let state = handle.state::<code::Code>();
            let _ = stop.set_enabled(n > 0);
            let _ = stop.set_text(if n > 0 { format!("Остановить команды ({n})") } else { "Остановить команды".into() });
            let _ = icon.set_tooltip(Some(if n > 0 { format!("Fizmat — команд идёт: {n}") } else { "Fizmat".into() }));
            if tick % 20 == 0 {
                match code::me(&state) {
                    Some(me) => {
                        let name = me["plan"].as_str().unwrap_or("");
                        let _ = plan.set_text(if name.is_empty() { "Подписка".to_string() } else { format!("Подписка {name}") });
                        let _ = usage.set_text(match me["usage"].as_i64() {
                            Some(p) => format!("Расход: {p}%"),
                            None => "Расход: без предела".into(),
                        });
                    }
                    None if !code::paired(&state) => {
                        let _ = plan.set_text("Компьютер не привязан");
                        let _ = usage.set_text("Расход: —");
                    }
                    None => {}
                }
            }
            tick = tick.wrapping_add(1);
            std::thread::sleep(Duration::from_secs(3));
        }
    });
    Ok(())
}
