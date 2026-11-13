// Элементы окна через UI Automation (Windows) — чтобы модель находила кнопки, поля и пункты меню по
// названию, а не угадывала по картинке. read — дерево или поиск с координатами в пикселях снимка;
// act — нажать элемент или вписать значение без мыши и клавиатуры: так работает и в свёрнутом окне,
// и на заблокированном экране (у обычных программ Windows; Electron и браузеры своё дерево почти не дают).
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

/// Что сделать с элементом.
pub enum Act<'a> {
    Press,
    SetValue(&'a str),
}

/// Элементы окна с названием query: сначала точные совпадения, потом содержащие; role — фильтр по виду.
fn matching(walker: &UITreeWalker, root: &UIElement, query: &str, role_filter: Option<&str>) -> Vec<(UIElement, ControlType, String)> {
    let q = query.trim().to_lowercase();
    let mut exact = Vec::new();
    let mut partial = Vec::new();
    let started = Instant::now();
    let mut stack = vec![(root.clone(), 0usize)];
    while let Some((e, depth)) = stack.pop() {
        if started.elapsed() > BUDGET || exact.len() + partial.len() > 50 {
            break;
        }
        let kind = e.get_control_type().unwrap_or(ControlType::Custom);
        let name = e.get_name().unwrap_or_default();
        let lower = name.trim().to_lowercase();
        let fits = role_filter.map(|r| role(kind) == r.trim().to_lowercase()).unwrap_or(true);
        if fits && !lower.is_empty() {
            if lower == q {
                exact.push((e.clone(), kind, name.trim().to_string()));
            } else if lower.contains(&q) {
                partial.push((e.clone(), kind, name.trim().to_string()));
            }
        }
        if depth < MAX_DEPTH {
            // Дети в обратном порядке — со стека они выйдут в порядке дерева.
            let mut kids = Vec::new();
            let mut child = walker.get_first_child(&e).ok();
            while let Some(c) = child {
                child = walker.get_next_sibling(&c).ok();
                kids.push(c);
            }
            for c in kids.into_iter().rev() {
                stack.push((c, depth + 1));
            }
        }
    }
    exact.extend(partial);
    exact
}

fn press(e: &UIElement) -> Result<&'static str, String> {
    use uiautomation::patterns::{UIExpandCollapsePattern, UIInvokePattern, UILegacyIAccessiblePattern, UISelectionItemPattern, UITogglePattern};
    if let Ok(p) = e.get_pattern::<UIInvokePattern>() {
        if p.invoke().is_ok() {
            return Ok("invoked");
        }
    }
    if let Ok(p) = e.get_pattern::<UITogglePattern>() {
        if p.toggle().is_ok() {
            return Ok("toggled");
        }
    }
    if let Ok(p) = e.get_pattern::<UISelectionItemPattern>() {
        if p.select().is_ok() {
            return Ok("selected");
        }
    }
    if let Ok(p) = e.get_pattern::<UIExpandCollapsePattern>() {
        if p.expand().is_ok() {
            return Ok("expanded");
        }
    }
    if let Ok(p) = e.get_pattern::<UILegacyIAccessiblePattern>() {
        if p.do_default_action().is_ok() {
            return Ok("default action done");
        }
    }
    Err("this element cannot be pressed through UI Automation — click it at its coordinates instead".into())
}

/// Значение поля сейчас (Value pattern) — для проверки, что вписалось.
fn value_of(e: &UIElement) -> Option<String> {
    use uiautomation::patterns::UIValuePattern;
    e.get_pattern::<UIValuePattern>().ok().and_then(|p| p.get_value().ok())
}

/// Выбрать пункт выпадающего списка по тексту: раскрыть, найти пункт (в самом списке или в его окне-попапе), выбрать.
/// Текст выбранного в списке: Value, а если его нет — имя выбранного пункта.
fn shown_value(e: &UIElement) -> String {
    value_of(e).unwrap_or_default().trim().to_string()
}

/// Выбрать пункт выпадающего списка по тексту: раскрыть, найти пункт (в самом списке или в его окне-попапе), выбрать
/// и проверить, что список его показывает. Не взялось — Err("CLICK:x,y …"): программа щёлкнет пункт мышью.
fn pick(ua: &UIAutomation, e: &UIElement, value: &str, popups: &dyn Fn() -> Vec<isize>) -> Result<&'static str, String> {
    use uiautomation::patterns::{UIExpandCollapsePattern, UIInvokePattern, UISelectionItemPattern};
    let walker = ua.get_control_view_walker().map_err(|e| e.to_string())?;
    let expand = e.get_pattern::<UIExpandCollapsePattern>().ok();
    if let Some(x) = &expand {
        let _ = x.expand();
        std::thread::sleep(Duration::from_millis(350));
    }
    let mut roots = vec![e.clone()];
    for h in popups() {
        if let Ok(r) = ua.element_from_handle(Handle::from(h)) {
            roots.push(r);
        }
    }
    let want = value.trim().to_lowercase();
    let mut options: Vec<String> = Vec::new();
    for root in &roots {
        // Все пункты списка — и для выбора, и чтобы назвать их, если нужного нет.
        let items: Vec<(UIElement, ControlType, String)> = matching(&walker, root, "", None)
            .into_iter()
            .filter(|(_, k, _)| matches!(k, ControlType::ListItem | ControlType::MenuItem))
            .collect();
        for (_, _, n) in &items {
            if !options.contains(n) && options.len() < 30 {
                options.push(n.clone());
            }
        }
        let item = items.iter().find(|(_, _, n)| n.to_lowercase() == want).or_else(|| items.iter().find(|(_, _, n)| n.to_lowercase().contains(&want)));
        if let Some((it, _, name)) = item {
            let _ = it.get_pattern::<UISelectionItemPattern>().map(|p| p.select()).ok()
                .or_else(|| it.get_pattern::<UIInvokePattern>().map(|p| p.invoke()).ok());
            std::thread::sleep(Duration::from_millis(150));
            // Проверять после того, как список закрыт: Office откатывает выбор, сделанный не мышью.
            if let Some(x) = &expand {
                let _ = x.collapse();
            }
            std::thread::sleep(Duration::from_millis(250));
            if shown_value(e).to_lowercase() == name.to_lowercase() {
                return Ok("option selected");
            }
            // Не взялось — программа откроет список щелчком по нему и щёлкнет пункт (как человек).
            if let Ok(r) = e.get_bounding_rectangle() {
                let idx = items.iter().position(|(_, _, n)| n == name).unwrap_or(0);
                return Err(format!("CLICKLIST:{},{},{idx} {name}", r.get_left() + r.get_width() / 2, r.get_top() + r.get_height() / 2));
            }
        }
    }
    if let Some(x) = &expand {
        let _ = x.collapse();
    }
    let list = if options.is_empty() { String::new() } else { format!(" — the options are: «{}»", options.join("», «")) };
    Err(format!("no option «{}» in this list{list}", value.trim()))
}

fn set_value(ua: &UIAutomation, e: &UIElement, kind: ControlType, value: &str, popups: &dyn Fn() -> Vec<isize>) -> Result<&'static str, String> {
    use uiautomation::patterns::{UILegacyIAccessiblePattern, UIValuePattern};
    // Выпадающий список (у Office он не берёт Value) — выбором пункта.
    if kind == ControlType::ComboBox {
        let editable = e.get_pattern::<UIValuePattern>().map(|p| !p.is_readonly().unwrap_or(true)).unwrap_or(false);
        if !editable {
            return pick(ua, e, value, popups);
        }
    }
    let took = |e: &UIElement| value_of(e).map(|v| v.trim() == value.trim()).unwrap_or(true);
    if let Ok(p) = e.get_pattern::<UIValuePattern>() {
        if p.is_readonly().unwrap_or(false) {
            return Err("this field is read-only".into());
        }
        if p.set_value(value).is_ok() && took(e) {
            return Ok("value set");
        }
    }
    if let Ok(p) = e.get_pattern::<UILegacyIAccessiblePattern>() {
        if p.set_value(value).is_ok() && took(e) {
            return Ok("value set");
        }
    }
    if kind == ControlType::ComboBox {
        return pick(ua, e, value, popups);
    }
    Err("this field did not take the value through UI Automation — click into it, press ctrl+a and use type".into())
}

/// Нажать элемент или вписать значение. roots — где искать по порядку (открытые попапы, потом окно); index — какое из совпадений.
pub fn act(roots: &[isize], query: &str, role_filter: Option<&str>, index: usize, what: Act, popups: &dyn Fn() -> Vec<isize>) -> Result<String, String> {
    let ua = automation()?;
    let walker = ua.get_control_view_walker().map_err(|e| e.to_string())?;
    let mut found = Vec::new();
    for h in roots {
        if let Ok(root) = ua.element_from_handle(Handle::from(*h)) {
            found = matching(&walker, &root, query, role_filter);
            if !found.is_empty() {
                break;
            }
        }
    }
    if found.is_empty() {
        return Err(format!("no element named like «{}» in this window — look at the element list of a fresh screenshot", query.trim()));
    }
    let n = found.len();
    let (e, kind, name) = found.into_iter().nth(index.max(1) - 1).ok_or_else(|| format!("only {n} elements match «{}»", query.trim()))?;
    if !e.is_enabled().unwrap_or(true) {
        return Err(format!("{} «{name}» is disabled", role(kind)));
    }
    let done = match what {
        Act::Press => press(&e)?,
        Act::SetValue(v) => set_value(&ua, &e, kind, v, popups)?,
    };
    let more = if n > 1 { format!(" ({n} elements matched; this was #{}; pass index for another)", index.max(1)) } else { String::new() };
    Ok(format!("{} «{name}»: {done}.{more}", role(kind)))
}

/// Центр элемента с названием query на рабочем столе — чтобы щёлкнуть его мышью (left_click с `text`).
pub fn locate(roots: &[isize], query: &str, role_filter: Option<&str>, index: usize) -> Result<(i32, i32, String), String> {
    let ua = automation()?;
    let walker = ua.get_control_view_walker().map_err(|e| e.to_string())?;
    let mut found = Vec::new();
    for h in roots {
        if let Ok(root) = ua.element_from_handle(Handle::from(*h)) {
            found = matching(&walker, &root, query, role_filter)
                .into_iter()
                .filter(|(e, _, _)| !e.is_offscreen().unwrap_or(false) && e.get_bounding_rectangle().map(|r| r.get_width() > 0 && r.get_height() > 0).unwrap_or(false))
                .collect();
            if !found.is_empty() {
                break;
            }
        }
    }
    if found.is_empty() {
        return Err(format!("no visible element named like «{}» in this window — look at the element list of a fresh screenshot", query.trim()));
    }
    let n = found.len();
    let (e, kind, name) = found.into_iter().nth(index.max(1) - 1).ok_or_else(|| format!("only {n} visible elements match «{}»", query.trim()))?;
    let r = e.get_bounding_rectangle().map_err(|e| e.to_string())?;
    let more = if n > 1 { format!(" ({n} matched; this was #{})", index.max(1)) } else { String::new() };
    Ok((r.get_left() + r.get_width() / 2, r.get_top() + r.get_height() / 2, format!("{} «{name}»{more}", role(kind))))
}

/// Элементы, на которые можно действовать, — коротким списком к снимку экрана: сначала кнопки, поля, пункты,
/// потом подписи (в них видно состояние: число в калькуляторе, текст статуса). Только видимые на показанном экране.
const ACTIONABLE: [ControlType; 16] = [
    ControlType::Button, ControlType::Edit, ControlType::CheckBox, ControlType::RadioButton, ControlType::ComboBox, ControlType::MenuItem,
    ControlType::TabItem, ControlType::Hyperlink, ControlType::ListItem, ControlType::TreeItem, ControlType::SplitButton, ControlType::DataItem,
    ControlType::Document, ControlType::Slider, ControlType::Spinner, ControlType::HeaderItem,
];

/// Что в элементе сейчас: текст поля (чтобы не разглядывать мелкий шрифт на снимке), флажок вкл/выкл.
fn state(e: &UIElement, kind: ControlType) -> String {
    use uiautomation::patterns::{UITogglePattern, UIValuePattern};
    match kind {
        ControlType::Edit | ControlType::ComboBox | ControlType::Document | ControlType::DataItem | ControlType::ListItem => {
            let Ok(p) = e.get_pattern::<UIValuePattern>() else { return String::new() };
            let v = p.get_value().unwrap_or_default();
            if v.is_empty() && matches!(kind, ControlType::DataItem | ControlType::ListItem) {
                return String::new();
            }
            let max = if kind == ControlType::Document || v.contains('\n') { 400 } else { 120 };
            let short: String = v.chars().take(max).collect::<String>().replace('\r', "").replace('\n', "⏎");
            let more = if v.chars().count() > max { format!("… ({} chars)", v.chars().count()) } else { String::new() };
            format!(" = \"{short}{more}\"")
        }
        ControlType::CheckBox | ControlType::RadioButton => match e.get_pattern::<UITogglePattern>().and_then(|p| p.get_toggle_state()) {
            Ok(s) => format!(" [{}]", if format!("{s:?}").to_lowercase() == "on" { "on" } else { "off" }),
            Err(_) => String::new(),
        },
        _ => String::new(),
    }
}

/// Колонки строки списка: «Размер: 89 КБ; Тип: Файл "BIN"» — из её детей (поля и подписи с названием колонки).
fn columns(walker: &UITreeWalker, row: &UIElement, row_name: &str) -> String {
    let mut parts = Vec::new();
    let mut child = walker.get_first_child(row).ok();
    while let Some(c) = child {
        let kind = c.get_control_type().unwrap_or(ControlType::Custom);
        if matches!(kind, ControlType::Edit | ControlType::Text | ControlType::DataItem) {
            let col = c.get_name().unwrap_or_default().trim().to_string();
            let val = value_of(&c).unwrap_or_default().replace(['\u{200e}', '\u{200f}'], "").trim().to_string();
            if !col.is_empty() && !val.is_empty() && val != row_name && parts.len() < 6 {
                parts.push(format!("{col}: {}", val.chars().take(40).collect::<String>()));
            }
        }
        child = walker.get_next_sibling(&c).ok();
    }
    parts.join("; ")
}

pub fn brief(hwnd: isize, to_shot: &dyn Fn(i32, i32) -> Option<(i32, i32)>, limit: usize, budget: Duration) -> Result<(String, String), String> {
    let ua = automation()?;
    let root = ua.element_from_handle(Handle::from(hwnd)).map_err(|e| format!("the window cannot be read: {e}"))?;
    let walker = ua.get_control_view_walker().map_err(|e| e.to_string())?;
    let started = Instant::now();
    // Содержимое (поля, ячейки, пункты списков, флажки) — отдельно от кнопок и меню: в Office лента иначе съедает весь список.
    let (mut content, mut acts, mut texts): (Vec<String>, Vec<String>, Vec<String>) = (Vec::new(), Vec::new(), Vec::new());
    let mut seen = std::collections::HashSet::new();
    let mut cut = false;
    let mut stack = vec![(root, 0usize)];
    while let Some((e, depth)) = stack.pop() {
        if started.elapsed() > budget {
            cut = true;
            break;
        }
        if acts.len() + content.len() >= limit * 3 {
            cut = true;
            break;
        }
        let kind = e.get_control_type().unwrap_or(ControlType::Custom);
        let offscreen = e.is_offscreen().unwrap_or(false);
        let act = ACTIONABLE.contains(&kind);
        if (act || kind == ControlType::Text) && !offscreen {
            let name: String = e.get_name().unwrap_or_default().trim().replace('\n', " ").chars().take(80).collect();
            if !name.is_empty() || matches!(kind, ControlType::Edit | ControlType::Document) {
                if let Ok(r) = e.get_bounding_rectangle() {
                    if r.get_width() > 0 && r.get_height() > 0 {
                        if let Some((x, y)) = to_shot(r.get_left() + r.get_width() / 2, r.get_top() + r.get_height() / 2) {
                            let mut line = format!("{} «{name}» ({x},{y})", role(kind));
                            if !e.is_enabled().unwrap_or(true) {
                                line.push_str(" disabled");
                            }
                            let mut st = state(&e, kind);
                            // Строка списка с колонками (Проводник в виде «Таблица»): колонки — в ту же строку, «Размер: 89 КБ; …».
                            if kind == ControlType::ListItem {
                                let cols = columns(&walker, &e, &name);
                                if !cols.is_empty() {
                                    st = format!(" — {cols}");
                                }
                            }
                            // Пустая ячейка таблицы («B7») — не в список: их на экране сотни.
                            let empty_cell = kind == ControlType::DataItem && st.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric());
                            line.push_str(&st);
                            if !empty_cell && seen.insert(line.clone()) {
                                // Дерево папок слева — не содержимое: оно не должно вытеснять файлы списка.
                                let is_content = matches!(kind, ControlType::Edit | ControlType::ComboBox | ControlType::Document | ControlType::DataItem
                                    | ControlType::ListItem | ControlType::CheckBox | ControlType::RadioButton | ControlType::Slider | ControlType::Spinner);
                                if !act { texts.push(line) } else if is_content { content.push(line) } else { acts.push(line) }
                            }
                        }
                    }
                }
            }
        }
        // Внутрь кнопок и пунктов не идём: там их же подписи.
        if depth < MAX_DEPTH && !(act && !matches!(kind, ControlType::Document | ControlType::TreeItem | ControlType::DataItem)) {
            let mut kids = Vec::new();
            let mut child = walker.get_first_child(&e).ok();
            while let Some(c) = child {
                child = walker.get_next_sibling(&c).ok();
                kids.push(c);
            }
            for c in kids.into_iter().rev() {
                stack.push((c, depth + 1));
            }
        }
    }
    let total = content.len() + acts.len();
    let content_room = content.len().min(limit * 2 / 3).max(limit.saturating_sub(acts.len()));
    let mut out: Vec<String> = content.iter().take(content_room).cloned().collect();
    let acts_room = limit.saturating_sub(out.len());
    out.extend(acts.iter().take(acts_room).cloned());
    cut = cut || out.len() < total;
    let room = limit.saturating_sub(out.len()).max(12);
    out.extend(texts.into_iter().take(room));
    if cut {
        out.push("… (more elements: ui_find with a word from the one you need)".into());
    }
    let focus = ua
        .get_focused_element()
        .ok()
        .map(|f| {
            let name: String = f.get_name().unwrap_or_default().trim().chars().take(60).collect();
            let kind = f.get_control_type().unwrap_or(ControlType::Custom);
            format!("{} «{name}»{}", role(kind), state(&f, kind))
        })
        .unwrap_or_default();
    Ok((out.join("\n"), focus))
}

/// Что под точкой рабочего стола: «listitem «a.txt»» — чтобы модель видела, во что попал щелчок по координатам.
pub fn at(x: i32, y: i32) -> Option<String> {
    let ua = automation().ok()?;
    let e = ua.element_from_point(uiautomation::types::Point::new(x, y)).ok()?;
    let kind = e.get_control_type().unwrap_or(ControlType::Custom);
    let name: String = e.get_name().unwrap_or_default().trim().replace('\n', " ").chars().take(60).collect();
    // Безымянная рамка или текст внутри пункта — назвать пункт: подняться до именованного родителя.
    if name.is_empty() || kind == ControlType::Text {
        let walker = ua.get_control_view_walker().ok()?;
        let mut cur = e.clone();
        for _ in 0..3 {
            let Ok(p) = walker.get_parent(&cur) else { break };
            let pk = p.get_control_type().unwrap_or(ControlType::Custom);
            let pn: String = p.get_name().unwrap_or_default().trim().chars().take(60).collect();
            if !pn.is_empty() && ACTIONABLE.contains(&pk) {
                return Some(format!("{} «{pn}»", role(pk)));
            }
            cur = p;
        }
    }
    (!name.is_empty()).then(|| format!("{} «{name}»", role(kind)))
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
