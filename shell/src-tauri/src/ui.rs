// Дерево элементов окна через UI Automation (Windows) — чтобы модель находила кнопки, поля и пункты
// меню по названию, а не угадывала по картинке. Только чтение: нажимает модель по-прежнему
// действиями computer по координатам, которые отсюда и берёт (в пикселях последнего снимка).
//
// Chromium и Electron строят дерево страницы по первому запросу — если пришло почти пустое
// дерево с документом внутри, спрашиваем ещё раз через секунду.

#![cfg(windows)]

use std::time::{Duration, Instant};

use uiautomation::controls::ControlType;
use uiautomation::types::Handle;
use uiautomation::{UIAutomation, UIElement, UITreeWalker};

/// Сколько строк и времени на одно дерево.
const MAX_LINES: usize = 300;
const MAX_DEPTH: usize = 30;
const BUDGET: Duration = Duration::from_secs(8);

/// Элемент, который стоит показать, даже если у него нет названия.
fn useful(kind: ControlType) -> bool {
    matches!(
        kind,
        ControlType::Button | ControlType::Edit | ControlType::CheckBox | ControlType::RadioButton | ControlType::ComboBox
            | ControlType::MenuItem | ControlType::TabItem | ControlType::Hyperlink | ControlType::ListItem | ControlType::TreeItem
            | ControlType::Document | ControlType::Slider | ControlType::SplitButton | ControlType::DataItem
    )
}

fn role(kind: ControlType) -> String {
    let s = format!("{kind:?}");
    s.to_lowercase()
}

struct Walk<'a> {
    walker: UITreeWalker,
    /// Центр элемента на рабочем столе → пиксели снимка (None — вне показанного экрана).
    to_shot: &'a dyn Fn(i32, i32) -> Option<(i32, i32)>,
    find: Option<String>,
    lines: Vec<String>,
    seen: usize,
    started: Instant,
    cut: bool,
}

impl Walk<'_> {
    fn line(&self, e: &UIElement, kind: ControlType, name: &str, depth: usize) -> String {
        let mut s = format!("{}{}", "  ".repeat(depth.min(12)), role(kind));
        if !name.is_empty() {
            let short: String = name.chars().take(120).collect();
            s.push_str(&format!(" «{}»", short.replace('\n', " ")));
        }
        if !e.is_enabled().unwrap_or(true) {
            s.push_str(" [disabled]");
        }
        match e.get_bounding_rectangle() {
            Ok(r) if r.get_width() > 0 && r.get_height() > 0 && !e.is_offscreen().unwrap_or(false) => {
                let (cx, cy) = (r.get_left() + r.get_width() / 2, r.get_top() + r.get_height() / 2);
                match (self.to_shot)(cx, cy) {
                    Some((x, y)) => s.push_str(&format!(" @ ({x}, {y})")),
                    None => s.push_str(" (off the shown display)"),
                }
            }
            _ => s.push_str(" (not visible)"),
        }
        s
    }

    fn visit(&mut self, e: &UIElement, depth: usize) {
        if self.lines.len() >= MAX_LINES || self.started.elapsed() > BUDGET {
            self.cut = true;
            return;
        }
        self.seen += 1;
        let kind = e.get_control_type().unwrap_or(ControlType::Custom);
        let name = e.get_name().unwrap_or_default();
        let shown = match &self.find {
            Some(q) => name.to_lowercase().contains(q),
            None => !name.trim().is_empty() || useful(kind),
        };
        if shown {
            let line = self.line(e, kind, name.trim(), if self.find.is_some() { 0 } else { depth });
            self.lines.push(line);
        }
        if depth >= MAX_DEPTH {
            return;
        }
        // Безымянные рамки не добавляют отступа — дерево остаётся читаемым.
        let next = if shown { depth + 1 } else { depth };
        let mut child = self.walker.get_first_child(e).ok();
        while let Some(c) = child {
            self.visit(&c, next);
            if self.cut {
                return;
            }
            child = self.walker.get_next_sibling(&c).ok();
        }
    }
}

fn automation() -> Result<UIAutomation, String> {
    UIAutomation::new().or_else(|_| UIAutomation::new_direct()).map_err(|e| format!("UI Automation is unavailable: {e}"))
}

/// Дерево окна (find = None) или элементы, в названии которых есть find.
pub fn read(hwnd: isize, find: Option<&str>, to_shot: &dyn Fn(i32, i32) -> Option<(i32, i32)>) -> Result<(String, bool), String> {
    let ua = automation()?;
    let root = ua.element_from_handle(Handle::from(hwnd)).map_err(|e| format!("the window cannot be read: {e}"))?;
    let walker = ua.get_control_view_walker().map_err(|e| e.to_string())?;
    let find = find.map(|q| q.trim().to_lowercase()).filter(|q| !q.is_empty());
    let mut walk = Walk { walker, to_shot, find, lines: Vec::new(), seen: 0, started: Instant::now(), cut: false };
    walk.visit(&root, 0);

    // Страница Chromium/Electron ещё не построила своё дерево — второй заход.
    let thin = walk.find.is_none() && walk.seen < 25 && walk.lines.iter().any(|l| l.trim_start().starts_with("document"));
    if thin {
        std::thread::sleep(Duration::from_millis(1200));
        walk.lines.clear();
        walk.seen = 0;
        walk.started = Instant::now();
        walk.visit(&root, 0);
    }
    Ok((walk.lines.join("\n"), walk.cut))
}
