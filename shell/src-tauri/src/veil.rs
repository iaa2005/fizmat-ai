// Сеанс управления компьютером — виден человеку и невидим модели.
//
// С первого действия Computer Use: по краю экрана светится рамка, по центру на несколько секунд —
// плашка «Fizmat использует Ваш компьютер» (dist/veil.html), а окно Fizmat уменьшается в правый
// нижний угол и встаёт поверх окон — там видно, что делает модель, и её можно остановить.
// Оба окна исключены из снимков экрана (WDA_EXCLUDEFROMCAPTURE): модель видит экран без них.
// Покров прозрачен для мыши — щелчки уходят в окна под ним.
// Тишина дольше IDLE (или выключатель в трее) — рамка гаснет, окно Fizmat возвращается на место.

#![cfg(windows)]

use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, GetWindowPlacement, IsWindowVisible, SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPlacement, SetWindowPos, ShowWindow,
    GWL_EXSTYLE, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_HIDE, SW_SHOWMAXIMIZED,
    SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WINDOWPLACEMENT, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};

/// Сколько тишины считается концом сеанса.
const IDLE: Duration = Duration::from_secs(20);
/// Окно Fizmat в углу (логические пиксели) и отступ от края.
const CORNER: (f64, f64) = (420.0, 600.0);
const GAP: f64 = 16.0;

struct Session {
    last: Instant,
    display: RECT,
    /// Как было окно Fizmat до сеанса: место, размер, развёрнуто ли, видно ли.
    main: Option<(isize, WINDOWPLACEMENT, bool)>,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

fn hwnd(w: &tauri::WebviewWindow) -> Option<HWND> {
    w.hwnd().ok().map(|h| h.0 as HWND)
}

fn same(a: &RECT, b: &RECT) -> bool {
    (a.left, a.top, a.right, a.bottom) == (b.left, b.top, b.right, b.bottom)
}

/// Выполнить на главном потоке окон и дождаться (окна и их стили — его хозяйство).
fn on_main<T: Send + 'static>(app: &AppHandle, f: impl FnOnce(&AppHandle) -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let _ = tx.send(f(&handle));
    })
    .ok()?;
    rx.recv_timeout(Duration::from_secs(3)).ok()
}

/// Перед каждым действием модели. Первое — начинает сеанс; дальше только продлевает
/// (и переносит рамку, если модель перешла на другой экран).
pub fn touch(app: &AppHandle, display: RECT) {
    let moved = {
        let mut guard = SESSION.lock().unwrap();
        match guard.as_mut() {
            Some(s) => {
                s.last = Instant::now();
                let moved = !same(&s.display, &display);
                s.display = display;
                Some(moved)
            }
            None => None,
        }
    };
    if let Some(moved) = moved {
        if moved {
            let app2 = app.clone();
            on_main(app, move |_| place_veil(&app2, display));
        }
        return;
    }
    // Окно-покров строится не на главном потоке (Tauri сам перешлёт его туда и дождётся).
    let fresh = app.get_webview_window("veil").is_none();
    if fresh {
        let built = WebviewWindowBuilder::new(app, "veil", WebviewUrl::App("veil.html".into()))
            .title("Fizmat")
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .focused(false)
            .visible(false)
            .build();
        if let Err(e) = built {
            crate::code::note("veil", &format!("build: {e}"));
        }
    }
    let app2 = app.clone();
    let main = on_main(app, move |_| {
        place_veil(&app2, display);
        if !fresh {
            if let Some(w) = app2.get_webview_window("veil") {
                let _ = w.eval("window.fzStart && window.fzStart()");
            }
        }
        shrink_main(&app2, display)
    })
    .flatten();
    *SESSION.lock().unwrap() = Some(Session { last: Instant::now(), display, main });

    // Сторож: тишина — конец сеанса.
    let app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        let idle = SESSION.lock().unwrap().as_ref().map(|s| s.last.elapsed() > IDLE);
        match idle {
            None => return,
            Some(true) => return finish(&app),
            Some(false) => {}
        }
    });
}

/// Покров на весь экран: сквозной для мыши, не берёт фокус, не попадает в снимки.
fn place_veil(app: &AppHandle, r: RECT) {
    let Some(w) = app.get_webview_window("veil") else { return };
    let Some(h) = hwnd(&w) else { return };
    unsafe {
        // Исключить из снимков — до того, как окно станет слоистым: слоистому Windows отказывает.
        let first = SetWindowDisplayAffinity(h, WDA_EXCLUDEFROMCAPTURE);
        let _ = w.set_ignore_cursor_events(true);
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        SetWindowLongPtrW(h, GWL_EXSTYLE, ex | (WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) as isize);
        if first == 0 && SetWindowDisplayAffinity(h, WDA_EXCLUDEFROMCAPTURE) == 0 {
            crate::code::note("veil", &format!("affinity refused (err {}): the frame may show in screenshots", windows_sys::Win32::Foundation::GetLastError()));
        }
        SetWindowPos(h, HWND_TOPMOST, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
}

/// Окно Fizmat — в правый нижний угол рабочей области, поверх окон, без фокуса.
/// Возвращает, как оно стояло, — чтобы вернуть.
fn shrink_main(app: &AppHandle, display: RECT) -> Option<(isize, WINDOWPLACEMENT, bool)> {
    let w = app.get_webview_window("main")?;
    let h = hwnd(&w)?;
    let scale = w.scale_factor().unwrap_or(1.0);
    unsafe {
        let mut was: WINDOWPLACEMENT = std::mem::zeroed();
        was.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        GetWindowPlacement(h, &mut was);
        let visible = IsWindowVisible(h) != 0;

        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let area = if GetMonitorInfoW(MonitorFromRect(&display, MONITOR_DEFAULTTONEAREST), &mut info) != 0 { info.rcWork } else { display };
        let (cw, ch) = ((CORNER.0 * scale) as i32, (CORNER.1 * scale) as i32);
        let ch = ch.min(area.bottom - area.top - (2.0 * GAP * scale) as i32);
        let gap = (GAP * scale) as i32;

        SetWindowDisplayAffinity(h, WDA_EXCLUDEFROMCAPTURE);
        // Развёрнутое или свёрнутое — сначала в обычное, без фокуса.
        let mut normal = was;
        normal.showCmd = SW_SHOWNOACTIVATE as u32;
        normal.rcNormalPosition = RECT { left: area.right - gap - cw, top: area.bottom - gap - ch, right: area.right - gap, bottom: area.bottom - gap };
        SetWindowPlacement(h, &normal);
        SetWindowPos(h, HWND_TOPMOST, area.right - gap - cw, area.bottom - gap - ch, cw, ch, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        Some((h as isize, was, visible))
    }
}

/// Конец сеанса: рамка гаснет, окно Fizmat — где было.
pub fn finish(app: &AppHandle) {
    let Some(s) = SESSION.lock().unwrap().take() else { return };
    if let Some(w) = app.get_webview_window("veil") {
        let _ = w.eval("window.fzEnd && window.fzEnd()");
    }
    let app2 = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        // Пока гасло, мог начаться новый сеанс — тогда покров нужен.
        if SESSION.lock().unwrap().is_some() {
            return;
        }
        on_main(&app2, move |app| {
            if let Some(h) = app.get_webview_window("veil").as_ref().and_then(hwnd) {
                // Окно показано мимо Tauri (SetWindowPos) — и прячем мимо него.
                unsafe {
                    SetWindowPos(h, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE);
                    ShowWindow(h, SW_HIDE);
                }
            }
            if let Some((h, mut was, visible)) = s.main {
                let h = h as HWND;
                unsafe {
                    SetWindowDisplayAffinity(h, WDA_NONE);
                    SetWindowPos(h, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE);
                    was.showCmd = if !visible {
                        SW_HIDE as u32
                    } else if was.showCmd == SW_SHOWMAXIMIZED as u32 {
                        SW_SHOWMAXIMIZED as u32
                    } else if was.showCmd == SW_SHOWMINNOACTIVE as u32 || was.showCmd == 2 {
                        SW_SHOWMINNOACTIVE as u32
                    } else {
                        SW_SHOWNOACTIVATE as u32
                    };
                    SetWindowPlacement(h, &was);
                }
            }
        });
    });
}

/// Окно Fizmat в углу — его не видно на снимках, но щелчок туда попал бы в него.
pub fn covers(x: i32, y: i32) -> bool {
    let s = SESSION.lock().unwrap();
    let Some((h, _, _)) = s.as_ref().and_then(|s| s.main) else { return false };
    let mut r: RECT = unsafe { std::mem::zeroed() };
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(h as HWND, &mut r) };
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}
