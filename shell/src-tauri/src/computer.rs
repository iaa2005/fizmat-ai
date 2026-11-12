// Управление компьютером (Computer Use): снимок экрана, мышь, клавиатура, буфер обмена,
// окна — руками модели, по набору действий Claude (screenshot, zoom, left_click, type, key…).
//
// Три замка, и все — здесь, в программе, а не на сайте:
// 1. Общий выключатель «Управление компьютером» (трей). Выключен — ни одного действия.
// 2. Приложения: сайт присылает список разрешённых человеком в чате (apps). Щелчок идёт
//    только в окно разрешённого приложения, ввод — только когда оно впереди.
// 3. Уровни, как у Claude: браузеры — только смотреть (иначе модель нажала бы «Разрешить»
//    на странице fizmat.ai сама себе), терминалы и редакторы кода — только щелчки (для
//    команд есть local_bash с разрешениями), остальное — полностью. Окно самой программы
//    Fizmat недоступно всегда.
//
// Координаты модели — в пикселях последнего снимка: он уменьшен, масштаб помнит VIEW.

#![cfg(windows)]

use std::ptr::null_mut;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};
use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, EnumDisplayMonitors, GetDC, GetDIBits, GetMonitorInfoW,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, CAPTUREBLT, DIB_RGB_COLORS, HDC, HMONITOR, MONITORINFO, SRCCOPY,
};
use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData};
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::System::Threading::{GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, VkKeyScanW, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetCursorPos, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, SetCursorPos, SetForegroundWindow, ShowWindow, WindowFromPoint, GA_ROOT, SW_RESTORE,
};

/// Длинная сторона снимка, который видит модель.
const LONG_EDGE: i32 = 1440;
const CF_UNICODETEXT: u32 = 13;

/// Экран последнего снимка: где он на рабочем столе и во сколько раз уменьшен.
#[derive(Clone, Copy)]
struct View {
    display: usize,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    scale: f64,
}

static VIEW: Mutex<Option<View>> = Mutex::new(None);

type Out = Result<Value, String>;

fn ok(text: impl Into<String>) -> Out {
    Ok(json!({ "ok": true, "text": text.into(), "meta": {} }))
}

// ── Экраны и снимок ──────────────────────────────────────────────────────

unsafe extern "system" fn on_monitor(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> i32 {
    let list = &mut *(data as *mut Vec<(RECT, bool)>);
    let mut info: MONITORINFO = std::mem::zeroed();
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(m, &mut info) != 0 {
        list.push((info.rcMonitor, info.dwFlags & 1 == 1));
    }
    1
}

/// Экраны: основной — первым.
fn displays() -> Vec<RECT> {
    let mut list: Vec<(RECT, bool)> = Vec::new();
    unsafe { EnumDisplayMonitors(null_mut(), std::ptr::null(), Some(on_monitor), &mut list as *mut _ as LPARAM) };
    list.sort_by_key(|(r, primary)| (!*primary, r.left, r.top));
    list.into_iter().map(|(r, _)| r).collect()
}

fn view() -> Result<View, String> {
    if let Some(v) = *VIEW.lock().unwrap() {
        return Ok(v);
    }
    use_display(0)
}

fn use_display(index: usize) -> Result<View, String> {
    let all = displays();
    let r = all.get(index).ok_or_else(|| format!("no display {} — there are {}", index + 1, all.len()))?;
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    let scale = (w.max(h) as f64 / LONG_EDGE as f64).max(1.0);
    let v = View { display: index, x: r.left, y: r.top, w, h, scale };
    *VIEW.lock().unwrap() = Some(v);
    Ok(v)
}

/// Кусок рабочего стола как RGB, сверху вниз.
fn capture(x: i32, y: i32, w: i32, h: i32) -> Result<Vec<u8>, String> {
    if w <= 0 || h <= 0 {
        return Err("empty region".into());
    }
    unsafe {
        let screen = GetDC(null_mut());
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp as _);
        let copied = BitBlt(mem, 0, 0, w, h, screen, x, y, SRCCOPY | CAPTUREBLT);
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = w;
        info.bmiHeader.biHeight = -h;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        let mut bgra = vec![0u8; (w as usize) * (h as usize) * 4];
        let rows = GetDIBits(mem, bmp, 0, h as u32, bgra.as_mut_ptr() as _, &mut info, DIB_RGB_COLORS);
        SelectObject(mem, old);
        DeleteObject(bmp as _);
        DeleteDC(mem);
        ReleaseDC(null_mut(), screen);
        if copied == 0 || rows == 0 {
            return Err("the screen could not be captured: Windows is not drawing it now — the session is shown over Remote Desktop with its window minimized or disconnected, the display is off, or a protected window is in front. ui_tree / ui_find / ui_press / ui_set_value still work; for the picture ask the person to restore the Remote Desktop window or turn on «Не гасить экран» in the Fizmat settings".into());
        }
        let mut rgb = Vec::with_capacity((w as usize) * (h as usize) * 3);
        for px in bgra.chunks_exact(4) {
            rgb.extend_from_slice(&[px[2], px[1], px[0]]);
        }
        Ok(rgb)
    }
}

/// Уменьшить усреднением по площади: текст остаётся читаемым.
fn shrink(rgb: &[u8], w: usize, h: usize, nw: usize, nh: usize) -> Vec<u8> {
    if nw == w && nh == h {
        return rgb.to_vec();
    }
    let mut out = vec![0u8; nw * nh * 3];
    for ny in 0..nh {
        let (y0, y1) = (ny * h / nh, (((ny + 1) * h + nh - 1) / nh).min(h).max(ny * h / nh + 1));
        for nx in 0..nw {
            let (x0, x1) = (nx * w / nw, (((nx + 1) * w + nw - 1) / nw).min(w).max(nx * w / nw + 1));
            let mut sum = [0u32; 3];
            for yy in y0..y1 {
                let row = yy * w * 3;
                for xx in x0..x1 {
                    let i = row + xx * 3;
                    sum[0] += rgb[i] as u32;
                    sum[1] += rgb[i + 1] as u32;
                    sum[2] += rgb[i + 2] as u32;
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let o = (ny * nw + nx) * 3;
            out[o] = (sum[0] / n) as u8;
            out[o + 1] = (sum[1] / n) as u8;
            out[o + 2] = (sum[2] / n) as u8;
        }
    }
    out
}

fn png(rgb: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut bytes, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut writer = enc.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(rgb).map_err(|e| e.to_string())?;
    }
    Ok(bytes)
}

fn picture(x: i32, y: i32, w: i32, h: i32, scale: f64, note: String) -> Out {
    let rgb = capture(x, y, w, h)?;
    let (nw, nh) = (((w as f64 / scale).round() as usize).max(1), ((h as f64 / scale).round() as usize).max(1));
    let small = shrink(&rgb, w as usize, h as usize, nw, nh);
    let data = png(&small, nw as u32, nh as u32)?;
    Ok(json!({ "ok": true, "text": note, "meta": { "image": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data),
        "width": nw, "height": nh } }))
}

/// Что сказать модели, когда Windows на экране блокировки.
const LOCKED: &str = "THE COMPUTER IS LOCKED (Windows lock screen): screenshots show only the lock screen and clicks, typing and the clipboard do not reach applications. What still works: ui_tree / ui_find to read windows and ui_press / ui_set_value to press and fill their elements in granted regular Windows programs (not browsers or Electron apps). For anything else tell the person to unlock it (you never enter PINs or passwords) — do not look for other workarounds";

/// Экран блокировки: ввод уходит на защищённый рабочий стол или впереди LockApp.
pub fn locked() -> bool {
    use windows_sys::Win32::System::StationsAndDesktops::{CloseDesktop, GetUserObjectInformationW, OpenInputDesktop, DESKTOP_READOBJECTS, UOI_NAME};
    unsafe {
        let d = OpenInputDesktop(0, 0, DESKTOP_READOBJECTS);
        if d.is_null() {
            return true;
        }
        let mut buf = [0u16; 64];
        let mut need = 0u32;
        let got = GetUserObjectInformationW(d as _, UOI_NAME, buf.as_mut_ptr() as _, (buf.len() * 2) as u32, &mut need);
        CloseDesktop(d);
        let name = String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(0)]);
        if got != 0 && !name.eq_ignore_ascii_case("default") {
            return true;
        }
    }
    front().map(|(_, app, _)| app == "lockapp" || app == "logonui").unwrap_or(false)
}

fn screenshot() -> Out {
    let v = view()?;
    if locked() {
        // Снимок экрана блокировки модели ничего не даст (а часто и не снимается вовсе).
        return ok(LOCKED);
    }
    let cursor = cursor().map(|(x, y)| format!(" The mouse is at ({x}, {y}).")).unwrap_or_default();
    let front = front().map(|(_, name, title)| format!(" Frontmost: {name} — «{title}».")).unwrap_or_default();
    let (nw, nh) = ((v.w as f64 / v.scale).round() as i32, (v.h as f64 / v.scale).round() as i32);
    picture(v.x, v.y, v.w, v.h, v.scale, format!("Screenshot of display {} of {}, {nw}×{nh}; coordinates for clicks are in this image's pixels.{cursor}{front}",
        v.display + 1, displays().len()))
}

fn zoom(args: &Value) -> Out {
    let v = view()?;
    let r: Vec<f64> = args["region"].as_array().map(|a| a.iter().filter_map(|n| n.as_f64()).collect()).unwrap_or_default();
    if r.len() != 4 || r[2] <= r[0] || r[3] <= r[1] {
        return Err("region must be [x0, y0, x1, y1] in the screenshot's pixels".into());
    }
    let (x0, y0) = real(&v, r[0], r[1]);
    let (x1, y1) = real(&v, r[2], r[3]);
    let (w, h) = ((x1 - x0).max(1), (y1 - y0).max(1));
    let scale = (w.max(h) as f64 / LONG_EDGE as f64).max(1.0);
    picture(x0, y0, w, h, scale, "Zoomed region at full resolution. Coordinates for clicks stay in the full screenshot's frame.".into())
}

// ── Окна и приложения ────────────────────────────────────────────────────

fn real(v: &View, x: f64, y: f64) -> (i32, i32) {
    (v.x + (x * v.scale).round() as i32, v.y + (y * v.scale).round() as i32)
}

fn point(args: &Value, key: &str) -> Result<(i32, i32), String> {
    let v = view()?;
    let c = args[key].as_array().filter(|a| a.len() == 2).ok_or_else(|| format!("{key} must be [x, y] in the screenshot's pixels"))?;
    let (x, y) = (c[0].as_f64().unwrap_or(-1.0), c[1].as_f64().unwrap_or(-1.0));
    if x < 0.0 || y < 0.0 || x > v.w as f64 / v.scale || y > v.h as f64 / v.scale {
        return Err(format!("{key} ({x}, {y}) is outside the screenshot — take a new screenshot"));
    }
    Ok(real(&v, x, y))
}

fn cursor() -> Option<(i32, i32)> {
    let v = view().ok()?;
    let mut p = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut p) } == 0 {
        return None;
    }
    Some((((p.x - v.x) as f64 / v.scale).round() as i32, ((p.y - v.y) as f64 / v.scale).round() as i32))
}

/// Программа окна: (pid, имя файла без .exe, строчными).
fn owner(hwnd: HWND) -> (u32, String) {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return (pid, String::new());
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let got = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        if got == 0 {
            return (pid, String::new());
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        let name = std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
        (pid, name)
    }
}

fn title(hwnd: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(hwnd);
        if n <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        String::from_utf16_lossy(&buf[..got.max(0) as usize])
    }
}

/// Окно впереди: (pid, программа, заголовок).
fn front() -> Option<(u32, String, String)> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return None;
    }
    let (pid, name) = owner(hwnd);
    Some((pid, name, title(hwnd)))
}

unsafe extern "system" fn on_window(hwnd: HWND, data: LPARAM) -> i32 {
    let list = &mut *(data as *mut Vec<(HWND, String, String)>);
    if IsWindowVisible(hwnd) != 0 && GetWindowTextLengthW(hwnd) > 0 {
        let (_, name) = owner(hwnd);
        if !name.is_empty() {
            list.push((hwnd, name, title(hwnd)));
        }
    }
    1
}

fn windows() -> Vec<(HWND, String, String)> {
    let mut list: Vec<(HWND, String, String)> = Vec::new();
    unsafe { EnumWindows(Some(on_window), &mut list as *mut _ as LPARAM) };
    list
}

/// Уровень приложения, как у Claude: read — только смотреть, click — только щелчки, full — всё.
fn tier(app: &str) -> &'static str {
    const BROWSERS: [&str; 12] = ["chrome", "msedge", "firefox", "opera", "brave", "browser", "vivaldi", "iexplore", "arc", "yandex", "chromium", "msedgewebview2"];
    const CODE: [&str; 22] = ["windowsterminal", "cmd", "powershell", "pwsh", "conhost", "openconsole", "wt", "code", "cursor", "devenv", "idea64",
        "pycharm64", "webstorm64", "phpstorm64", "clion64", "rider64", "goland64", "sublime_text", "notepad++", "mintty", "alacritty", "wezterm-gui"];
    if BROWSERS.contains(&app) {
        "read"
    } else if CODE.contains(&app) {
        "click"
    } else {
        "full"
    }
}

fn granted(args: &Value, app: &str) -> bool {
    args["apps"].as_array().map(|a| a.iter().any(|x| x.as_str().map(|s| s.to_lowercase()) == Some(app.to_string()))).unwrap_or(false)
}

/// Можно ли действовать в окне: need — "click" или "type".
fn allow(args: &Value, pid: u32, app: &str, need: &str) -> Result<(), String> {
    if pid == unsafe { GetCurrentProcessId() } || app == "fizmat" || app == "fizmat-guard" {
        return Err("the Fizmat app itself cannot be controlled".into());
    }
    if app.is_empty() {
        return Err("this window belongs to a protected system process and cannot be controlled".into());
    }
    if !granted(args, app) {
        return Err(format!("the application «{app}» is not granted in this chat — ask the person with computer_request_access (apps: [\"{app}\"])"));
    }
    match (tier(app), need) {
        ("read", _) => Err(format!("«{app}» is a browser: it is view-only here (screenshots), clicks and typing are blocked. Use web tools for the web")),
        ("click", "type") => Err(format!("«{app}» is a terminal or code editor: typing and keys are blocked here. Run commands with local_bash, edit files with local_edit")),
        _ => Ok(()),
    }
}

/// Экран, с которым модель работает сейчас, — для рамки (veil.rs).
pub fn display() -> Option<RECT> {
    let v = view().ok()?;
    Some(RECT { left: v.x, top: v.y, right: v.x + v.w, bottom: v.y + v.h })
}

fn allow_at(args: &Value, x: i32, y: i32) -> Result<(), String> {
    if crate::veil::covers(x, y) {
        return Err("this spot is under the Fizmat window: while you control the computer it sits in the bottom-right corner on top of other windows and is hidden from screenshots. Move the target window or use another spot".into());
    }
    let hwnd = unsafe { GetAncestor(WindowFromPoint(POINT { x, y }), GA_ROOT) };
    if hwnd.is_null() {
        return Err("there is no window at this point".into());
    }
    let (pid, app) = owner(hwnd);
    allow(args, pid, &app, "click")
}

fn allow_front(args: &Value) -> Result<(), String> {
    let (pid, app, _) = front().ok_or("no window is in front")?;
    allow(args, pid, &app, "type")
}

// ── Мышь и клавиатура ────────────────────────────────────────────────────

fn mouse(flags: u32, data: i32) {
    let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 } } };
    unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) };
}

fn pause(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

fn move_to(x: i32, y: i32) {
    unsafe { SetCursorPos(x, y) };
    pause(40);
}

fn key_event(vk: u16, scan: u16, flags: u32) {
    let input = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } } };
    unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) };
}

/// Имя клавиши → (код, расширенная ли).
fn vk(name: &str) -> Option<(u16, bool)> {
    let n = name.trim().to_lowercase();
    let known = match n.as_str() {
        "ctrl" | "control" => (0x11, false),
        "alt" | "option" => (0x12, false),
        "shift" => (0x10, false),
        "win" | "windows" | "cmd" | "super" | "meta" => (0x5B, true),
        "enter" | "return" => (0x0D, false),
        "tab" => (0x09, false),
        "esc" | "escape" => (0x1B, false),
        "space" => (0x20, false),
        "backspace" => (0x08, false),
        "delete" | "del" => (0x2E, true),
        "insert" | "ins" => (0x2D, true),
        "home" => (0x24, true),
        "end" => (0x23, true),
        "pageup" | "page_up" | "pgup" => (0x21, true),
        "pagedown" | "page_down" | "pgdn" => (0x22, true),
        "up" | "arrowup" => (0x26, true),
        "down" | "arrowdown" => (0x28, true),
        "left" | "arrowleft" => (0x25, true),
        "right" | "arrowright" => (0x27, true),
        "printscreen" | "print_screen" => (0x2C, true),
        "capslock" => (0x14, false),
        "apps" | "menu" => (0x5D, true),
        _ => (0, false),
    };
    if known.0 != 0 {
        return Some(known);
    }
    if let Some(num) = n.strip_prefix('f').and_then(|d| d.parse::<u16>().ok()).filter(|d| (1..=24).contains(d)) {
        return Some((0x70 + num - 1, false));
    }
    let mut chars = n.chars();
    let (c, rest) = (chars.next()?, chars.next());
    if rest.is_some() {
        return None;
    }
    let mut unit = [0u16; 2];
    let code = unsafe { VkKeyScanW(c.encode_utf16(&mut unit)[0]) };
    (code != -1).then_some(((code & 0xFF) as u16, false))
}

/// «ctrl+shift+s» → клавиши по порядку.
fn chord(text: &str) -> Result<Vec<(u16, bool)>, String> {
    text.split('+').map(|k| vk(k).ok_or_else(|| format!("unknown key «{k}»"))).collect()
}

fn press(keys: &[(u16, bool)], hold: Duration) {
    for (code, ext) in keys {
        key_event(*code, 0, if *ext { KEYEVENTF_EXTENDEDKEY } else { 0 });
        pause(12);
    }
    std::thread::sleep(hold);
    for (code, ext) in keys.iter().rev() {
        key_event(*code, 0, KEYEVENTF_KEYUP | if *ext { KEYEVENTF_EXTENDEDKEY } else { 0 });
        pause(12);
    }
}

fn with_modifiers<T>(args: &Value, act: impl FnOnce() -> T) -> Result<T, String> {
    let mods = match args["modifiers"].as_str().filter(|m| !m.is_empty()) {
        Some(m) => chord(m)?,
        None => Vec::new(),
    };
    for (code, ext) in &mods {
        key_event(*code, 0, if *ext { KEYEVENTF_EXTENDEDKEY } else { 0 });
    }
    let out = act();
    for (code, ext) in mods.iter().rev() {
        key_event(*code, 0, KEYEVENTF_KEYUP | if *ext { KEYEVENTF_EXTENDEDKEY } else { 0 });
    }
    Ok(out)
}

fn click(args: &Value, down: u32, up: u32, times: usize) -> Out {
    let (x, y) = point(args, "coordinate")?;
    allow_at(args, x, y)?;
    move_to(x, y);
    with_modifiers(args, || {
        for i in 0..times {
            if i > 0 {
                pause(70);
            }
            mouse(down, 0);
            pause(25);
            mouse(up, 0);
        }
    })?;
    pause(120);
    ok("Clicked.")
}

fn type_text(args: &Value) -> Out {
    let text = args["text"].as_str().unwrap_or("");
    if text.is_empty() {
        return Err("text is empty".into());
    }
    allow_front(args)?;
    for c in text.chars() {
        match c {
            '\r' => {}
            '\n' => press(&[(0x0D, false)], Duration::ZERO),
            '\t' => press(&[(0x09, false)], Duration::ZERO),
            _ => {
                let mut unit = [0u16; 2];
                for u in c.encode_utf16(&mut unit) {
                    key_event(0, *u, KEYEVENTF_UNICODE);
                    key_event(0, *u, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP);
                }
                pause(6);
            }
        }
    }
    ok(format!("Typed {} characters.", text.chars().count()))
}

// ── Буфер обмена ─────────────────────────────────────────────────────────

fn clipboard_read() -> Out {
    unsafe {
        if OpenClipboard(null_mut()) == 0 {
            return Err("the clipboard is busy".into());
        }
        let h = GetClipboardData(CF_UNICODETEXT);
        let mut text = String::new();
        if !h.is_null() {
            let p = GlobalLock(h as _) as *const u16;
            if !p.is_null() {
                let mut len = 0usize;
                while *p.add(len) != 0 && len < 1_000_000 {
                    len += 1;
                }
                text = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
                GlobalUnlock(h as _);
            }
        }
        CloseClipboard();
        ok(if text.is_empty() { "(the clipboard has no text)".to_string() } else { text })
    }
}

fn clipboard_write(args: &Value) -> Out {
    let text = args["text"].as_str().unwrap_or("");
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(null_mut()) == 0 {
            return Err("the clipboard is busy".into());
        }
        EmptyClipboard();
        let h = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
        let p = GlobalLock(h) as *mut u16;
        if p.is_null() {
            CloseClipboard();
            return Err("no memory for the clipboard".into());
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
        GlobalUnlock(h);
        SetClipboardData(CF_UNICODETEXT, h as _);
        CloseClipboard();
    }
    ok("The clipboard is set.")
}

// ── Приложения ───────────────────────────────────────────────────────────

fn list_windows(args: &Value) -> Out {
    let me = unsafe { GetCurrentProcessId() };
    let mut seen: Vec<String> = Vec::new();
    let mut lines = Vec::new();
    for (hwnd, app, title) in windows() {
        if owner(hwnd).0 == me || seen.contains(&app) {
            continue;
        }
        seen.push(app.clone());
        let level = match tier(&app) { "read" => "a browser: view only even when granted", "click" => "terminal or code editor: clicks only when granted", _ => "full control when granted" };
        let state = if granted(args, &app) { "GRANTED in this chat" } else { "not granted — computer_request_access first" };
        lines.push(format!("{app} — «{title}» ({state}; {level})"));
    }
    if locked() {
        lines.insert(0, LOCKED.to_string());
    }
    Ok(json!({ "ok": true, "text": lines.join("\n"), "meta": { "apps": seen } }))
}

fn open_application(args: &Value) -> Out {
    let name = args["name"].as_str().unwrap_or("").trim().to_lowercase();
    let name = name.trim_end_matches(".exe").to_string();
    if name.is_empty() || name.contains(['/', '\\', '"', '&', '|', '>', '<', '^', '%', ' ']) {
        return Err("name must be a plain application name like «notepad» or «excel»".into());
    }
    if !granted(args, &name) {
        return Err(format!("the application «{name}» is not granted in this chat — ask the person with computer_request_access"));
    }
    if let Some((hwnd, _, title)) = windows().into_iter().find(|(_, app, _)| *app == name) {
        unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            SetForegroundWindow(hwnd);
        }
        pause(300);
        return ok(format!("«{name}» is in front: «{title}»."));
    }
    use std::os::windows::process::CommandExt;
    std::process::Command::new("cmd").args(["/C", "start", "", &name]).creation_flags(0x0800_0000).spawn().map_err(|e| format!("cannot start «{name}»: {e}"))?;
    pause(1500);
    ok(format!("Started «{name}». Take a screenshot to see its window."))
}

/// Окно для дерева: по имени программы или переднее. Окна самой Fizmat — нет.
fn target(args: &Value) -> Result<(HWND, String, String), String> {
    let name = args["name"].as_str().unwrap_or("").trim().trim_end_matches(".exe").to_lowercase();
    let found = if name.is_empty() {
        let hwnd = unsafe { GetForegroundWindow() };
        (!hwnd.is_null()).then(|| {
            let (_, app) = owner(hwnd);
            (hwnd, app, title(hwnd))
        })
    } else {
        windows().into_iter().find(|(_, app, _)| *app == name)
    };
    let (hwnd, app, title) = found.ok_or_else(|| if name.is_empty() { "no window is in front".to_string() } else { format!("«{name}» has no open window — see list_windows") })?;
    if owner(hwnd).0 == unsafe { GetCurrentProcessId() } || app == "fizmat" || app == "fizmat-guard" {
        return Err("this is the Fizmat app itself — it is not shown to the model".into());
    }
    Ok((hwnd, app, title))
}

/// UI Automation: элементы окна (кнопки, поля, пункты) с центрами в пикселях последнего снимка.
fn ui(args: &Value, find: bool) -> Out {
    let v = view()?;
    let (hwnd, app, title) = target(args)?;
    let query = if find {
        Some(args["text"].as_str().map(str::trim).filter(|s| !s.is_empty()).ok_or("ui_find needs `text` — a word from the element's name")?)
    } else {
        None
    };
    let to_shot = move |x: i32, y: i32| {
        (x >= v.x && y >= v.y && x < v.x + v.w && y < v.y + v.h)
            .then(|| ((((x - v.x) as f64) / v.scale).round() as i32, (((y - v.y) as f64) / v.scale).round() as i32))
    };
    let (text, cut) = crate::ui::read(hwnd as isize, query, &to_shot)?;
    let mut head = format!("UI of {app} — «{title}». Coordinates (x, y) are element centres in the latest screenshot's pixels: click them as they are.");
    if unsafe { IsIconic(hwnd) } != 0 {
        head.push_str(" The window is MINIMIZED: its elements are not on screen — bring it up with open_application first, then read the tree again.");
    }
    let body = match (text.is_empty(), query) {
        (true, Some(q)) => format!("Nothing named like «{q}» in this window. Try another word, ui_tree, or a screenshot."),
        (true, None) => "This window exposes no accessible elements — work from screenshots.".to_string(),
        (false, _) => text,
    };
    let tail = if cut { "\n(cut: the window has more elements — use ui_find with a word from the element you need)" } else { "" };
    let lock = if locked() { format!("\n{LOCKED}") } else { String::new() };
    ok(format!("{head}\n{body}{tail}{lock}"))
}

/// Нажать элемент или вписать значение через UI Automation — с теми же замками, что щелчок и ввод.
fn ui_act(args: &Value) -> Out {
    let (hwnd, app, title) = target(args)?;
    let press = args["action"] == "ui_press";
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    allow(args, pid, &app, if press { "click" } else { "type" })?;
    let query = args["text"].as_str().map(str::trim).filter(|s| !s.is_empty()).ok_or("`text` — the element's name, as ui_tree / ui_find show it")?;
    let index = args["index"].as_u64().unwrap_or(1).clamp(1, 50) as usize;
    let role = args["role"].as_str().filter(|s| !s.trim().is_empty());
    let what = if press {
        crate::ui::Act::Press
    } else {
        crate::ui::Act::SetValue(args["value"].as_str().ok_or("`value` — the text to put into the field")?)
    };
    let done = crate::ui::act(hwnd as isize, query, role, index, what)?;
    ok(format!("In {app} — «{title}»: {done}"))
}

// ── Вход ─────────────────────────────────────────────────────────────────

fn one(args: &Value) -> Out {
    let action = args["action"].as_str().unwrap_or("");
    // ui_press / ui_set_value идут мимо мыши и клавиатуры — им блокировка экрана не помеха.
    let input = !matches!(action, "screenshot" | "zoom" | "switch_display" | "cursor_position" | "list_windows" | "wait" | "ui_tree" | "ui_find"
        | "ui_press" | "ui_set_value" | "read_clipboard");
    if input && locked() {
        return Err(LOCKED.into());
    }
    match action {
        "screenshot" => screenshot(),
        "zoom" => zoom(args),
        "switch_display" => {
            let n = args["display"].as_u64().unwrap_or(1).max(1) as usize;
            use_display(n - 1)?;
            screenshot()
        }
        "cursor_position" => cursor().map(|(x, y)| ok(format!("The mouse is at ({x}, {y}) in the screenshot's pixels."))).unwrap_or_else(|| Err("no cursor".into())),
        "list_windows" => list_windows(args),
        "ui_tree" => ui(args, false),
        "ui_find" => ui(args, true),
        "ui_press" | "ui_set_value" => ui_act(args),
        "wait" => {
            std::thread::sleep(Duration::from_secs_f64(args["duration"].as_f64().unwrap_or(1.0).clamp(0.0, 10.0)));
            ok("Waited.")
        }
        "left_click" => click(args, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, 1),
        "double_click" => click(args, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, 2),
        "triple_click" => click(args, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, 3),
        "right_click" => click(args, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, 1),
        "middle_click" => click(args, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, 1),
        "mouse_move" => {
            let (x, y) = point(args, "coordinate")?;
            move_to(x, y);
            ok("Moved.")
        }
        "left_mouse_down" | "left_mouse_up" => {
            if !args["coordinate"].is_null() {
                let (x, y) = point(args, "coordinate")?;
                allow_at(args, x, y)?;
                move_to(x, y);
            } else {
                let mut p = POINT { x: 0, y: 0 };
                unsafe { GetCursorPos(&mut p) };
                allow_at(args, p.x, p.y)?;
            }
            mouse(if args["action"] == "left_mouse_down" { MOUSEEVENTF_LEFTDOWN } else { MOUSEEVENTF_LEFTUP }, 0);
            ok("Done.")
        }
        "left_click_drag" => {
            let (x0, y0) = point(args, "start_coordinate")?;
            let (x1, y1) = point(args, "coordinate")?;
            allow_at(args, x0, y0)?;
            allow_at(args, x1, y1)?;
            move_to(x0, y0);
            mouse(MOUSEEVENTF_LEFTDOWN, 0);
            pause(120);
            for i in 1..=12 {
                move_to(x0 + (x1 - x0) * i / 12, y0 + (y1 - y0) * i / 12);
            }
            pause(120);
            mouse(MOUSEEVENTF_LEFTUP, 0);
            ok("Dragged.")
        }
        "scroll" => {
            let (x, y) = point(args, "coordinate")?;
            allow_at(args, x, y)?;
            move_to(x, y);
            let amount = args["scroll_amount"].as_i64().unwrap_or(3).clamp(1, 30) as i32 * 120;
            match args["scroll_direction"].as_str().unwrap_or("down") {
                "up" => mouse(MOUSEEVENTF_WHEEL, amount),
                "down" => mouse(MOUSEEVENTF_WHEEL, -amount),
                "left" => mouse(MOUSEEVENTF_HWHEEL, -amount),
                "right" => mouse(MOUSEEVENTF_HWHEEL, amount),
                other => return Err(format!("unknown scroll_direction «{other}»")),
            }
            pause(150);
            ok("Scrolled.")
        }
        "type" => type_text(args),
        "key" => {
            allow_front(args)?;
            let text = args["text"].as_str().unwrap_or("");
            let repeat = args["repeat"].as_u64().unwrap_or(1).clamp(1, 100);
            for _ in 0..repeat {
                for combo in text.split_whitespace() {
                    press(&chord(combo)?, Duration::from_millis(20));
                    pause(30);
                }
            }
            ok("Pressed.")
        }
        "hold_key" => {
            allow_front(args)?;
            let keys = chord(args["text"].as_str().unwrap_or(""))?;
            press(&keys, Duration::from_secs_f64(args["duration"].as_f64().unwrap_or(1.0).clamp(0.0, 10.0)));
            ok("Held.")
        }
        "read_clipboard" => clipboard_read(),
        "write_clipboard" => clipboard_write(args),
        "open_application" => open_application(args),
        other => Err(format!("unknown action: {other}")),
    }
}

/// Вызов сайта: одно действие или пачка (action: "batch", actions: [...]) — по порядку, до первой ошибки.
/// stop — человек выключил управление или остановил ход.
pub fn run(args: &Value, enabled: bool, stop: &dyn Fn() -> bool) -> Out {
    if !enabled {
        return Err("computer control is switched off in the Fizmat app on this computer — the person turns it on in the tray menu («Управление компьютером»)".into());
    }
    if args["action"] != "batch" {
        return one(args);
    }
    let list = args["actions"].as_array().cloned().unwrap_or_default();
    let mut results = Vec::new();
    for (i, item) in list.iter().enumerate() {
        if stop() {
            results.push(json!({ "ok": false, "text": "stopped by the person" }));
            break;
        }
        let mut step = item.clone();
        step["apps"] = args["apps"].clone();
        match one(&step) {
            Ok(v) => results.push(json!({ "ok": true, "text": v["text"], "image": v["meta"]["image"], "width": v["meta"]["width"], "height": v["meta"]["height"] })),
            Err(e) => {
                results.push(json!({ "ok": false, "text": format!("action {} ({}) failed: {e}", i + 1, item["action"].as_str().unwrap_or("?")) }));
                break;
            }
        }
    }
    Ok(json!({ "ok": true, "text": "", "meta": { "results": results } }))
}
