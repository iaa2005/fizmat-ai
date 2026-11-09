// Появление строки быстрого вопроса (tray.rs): окно показывается невидимым и плавно
// проявляется прозрачностью целиком, а прячется, стерев строку, — прошлый кадр не
// мелькает перед анимацией страницы. Размытие фона Windows (acrylic) пробовали — на
// скруглённых углах вылезают уголки, человеку не понравилось.

use std::time::Duration;

use tauri::WebviewWindow;

#[cfg(windows)]
fn alpha(window: &WebviewWindow, value: u8) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetLayeredWindowAttributes, SetWindowLongPtrW, GWL_EXSTYLE, LWA_ALPHA, WS_EX_LAYERED,
    };
    let Ok(h) = window.hwnd() else { return };
    let h = h.0 as windows_sys::Win32::Foundation::HWND;
    unsafe {
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        if ex & WS_EX_LAYERED as isize == 0 {
            SetWindowLongPtrW(h, GWL_EXSTYLE, ex | WS_EX_LAYERED as isize);
        }
        SetLayeredWindowAttributes(h, 0, value, LWA_ALPHA);
    }
}

#[cfg(not(windows))]
fn alpha(_: &WebviewWindow, _: u8) {}

/// Показать: окно появляется невидимым, страница начинает анимацию из размытия, а окно
/// за ~200 мс проявляется целиком.
pub fn appear(window: &WebviewWindow) {
    alpha(window, 0);
    let _ = window.show();
    let _ = window.set_focus();
    let _ = window.eval("window.fzAppear && window.fzAppear()");
    let w = window.clone();
    std::thread::spawn(move || {
        // Кадр страницы в начале анимации должен успеть отрисоваться.
        std::thread::sleep(Duration::from_millis(30));
        const STEPS: u32 = 12;
        for i in 1..=STEPS {
            let t = i as f64 / STEPS as f64;
            let eased = 1.0 - (1.0 - t).powi(3);
            alpha(&w, (eased * 255.0).round() as u8);
            std::thread::sleep(Duration::from_millis(16));
        }
    });
}

/// Спрятать так, чтобы следующий показ начался с пустого кадра: Windows, показывая окно,
/// сперва рисует запомненный кадр (и сбрасывает прозрачность) — в нём строки уже быть не должно.
pub fn vanish(window: &WebviewWindow) {
    alpha(window, 0);
    let _ = window.eval("window.fzReset && window.fzReset()");
    let w = window.clone();
    std::thread::spawn(move || {
        // Страница должна успеть отрисовать пустой кадр.
        std::thread::sleep(Duration::from_millis(80));
        let _ = w.hide();
    });
}
