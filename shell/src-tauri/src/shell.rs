// Команды оболочки для Fizmat Code (local_bash): Git Bash, если он есть, иначе
// PowerShell. Команда идёт в папке чата, без окна консоли, весь процесс с потомками —
// в Job Object: таймаут и «Стоп» в чате убивают дерево целиком, а по концу команды
// не остаются висеть забытые процессы. Вывод (stdout и stderr вместе) урезается с
// середины: голова и хвост — самое нужное модели.

use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const HEAD: usize = 12 * 1024;
const TAIL: usize = 36 * 1024;

/// Потолок таймаута: фоновая команда — до шести часов.
pub const MAX_SECS: u64 = 6 * 3600;

/// «Остановить все команды» в трее: каждая команда помнит номер на старте и умирает, когда он сменился.
static GENERATION: AtomicU64 = AtomicU64::new(0);
static RUNNING: AtomicUsize = AtomicUsize::new(0);

pub fn stop_all() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// Сколько команд идёт сейчас — для подсказки в трее.
pub fn running() -> usize {
    RUNNING.load(Ordering::SeqCst)
}

struct Counted;

impl Counted {
    fn new() -> Self {
        RUNNING.fetch_add(1, Ordering::SeqCst);
        Counted
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        RUNNING.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Clone)]
pub struct Shell {
    pub name: &'static str,
    exe: PathBuf,
}

/// Оболочка компьютера: Git Bash из обычных мест установки или рядом с git.exe, иначе PowerShell.
pub fn detect() -> &'static Shell {
    static SHELL: OnceLock<Shell> = OnceLock::new();
    SHELL.get_or_init(|| {
        if !cfg!(windows) {
            return Shell { name: "bash", exe: PathBuf::from("/bin/bash") };
        }
        let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
        let mut places: Vec<PathBuf> = [env("ProgramFiles"), env("ProgramW6432"), env("ProgramFiles(x86)")]
            .into_iter().flatten().map(|p| p.join("Git").join("bin").join("bash.exe")).collect();
        if let Some(local) = env("LOCALAPPDATA") {
            places.push(local.join("Programs").join("Git").join("bin").join("bash.exe"));
        }
        // git.exe в PATH: …\Git\cmd\git.exe → …\Git\bin\bash.exe
        if let Some(path) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path) {
                if dir.join("git.exe").is_file() {
                    if let Some(git) = dir.parent() {
                        places.push(git.join("bin").join("bash.exe"));
                    }
                }
            }
        }
        match places.into_iter().find(|p| p.is_file()) {
            Some(exe) => Shell { name: "bash", exe },
            None => Shell { name: "powershell", exe: PathBuf::from("powershell.exe") },
        }
    })
}

/// Голова и хвост вывода; середина — счётчиком.
#[derive(Default)]
struct Output {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    dropped: usize,
}

impl Output {
    fn push(&mut self, bytes: &[u8]) {
        let room = HEAD.saturating_sub(self.head.len());
        let (a, b) = bytes.split_at(room.min(bytes.len()));
        self.head.extend_from_slice(a);
        for &c in b {
            if self.tail.len() == TAIL {
                self.tail.pop_front();
                self.dropped += 1;
            }
            self.tail.push_back(c);
        }
    }

    fn text(&self) -> String {
        let mut s = decode(&self.head);
        if self.dropped > 0 {
            s.push_str(&format!("\n… [{} bytes of output omitted] …\n", self.dropped));
        }
        let tail: Vec<u8> = self.tail.iter().copied().collect();
        s.push_str(&decode(&tail));
        s
    }
}

/// Вывод в UTF-8 — как есть; иначе это кодировка Windows: cp1251 (ANSI) у Python и
/// многих программ в трубу, cp866 (OEM) у консольных — PowerShell, cmd, net, ipconfig.
/// Из двух берём ту, где текст похож на русский: в чужой кодировке выходят «Ґ», «Є»,
/// псевдографика. Обрезка головы и хвоста может разрезать один символ UTF-8 — пара
/// битых байтов на краях ещё не повод менять кодировку.
fn decode(bytes: &[u8]) -> String {
    let bad: usize = bytes.utf8_chunks().map(|c| c.invalid().len()).sum();
    if bad <= 4 {
        return String::from_utf8_lossy(bytes).to_string();
    }
    let ansi = codepage(bytes, false);
    let oem = codepage(bytes, true);
    match (ansi, oem) {
        (Some(a), Some(o)) => if russian(&o) > russian(&a) { o } else { a },
        (Some(a), None) => a,
        (None, Some(o)) => o,
        (None, None) => String::from_utf8_lossy(bytes).to_string(),
    }
}

/// Насколько текст похож на русский: обычные буквы — плюс, буквы чужих алфавитов
/// кириллицы, псевдографика и редкие знаки из верхней половины таблицы — минус.
fn russian(s: &str) -> i64 {
    s.chars()
        .map(|c| match c {
            'а'..='я' | 'А'..='Я' | 'ё' | 'Ё' => 1,
            '\u{0400}'..='\u{04FF}' | '\u{2500}'..='\u{259F}' | '\u{2010}'..='\u{2122}' | '\u{00A0}'..='\u{00BF}' => -2,
            _ => 0,
        })
        .sum()
}

#[cfg(windows)]
fn codepage(bytes: &[u8], oem: bool) -> Option<String> {
    use windows_sys::Win32::Globalization::{MultiByteToWideChar, CP_ACP, CP_OEMCP};
    let cp = if oem { CP_OEMCP } else { CP_ACP };
    let len = i32::try_from(bytes.len()).ok()?;
    // SAFETY: буферы живые, длины переданы точно.
    let n = unsafe { MultiByteToWideChar(cp, 0, bytes.as_ptr(), len, std::ptr::null_mut(), 0) };
    if n <= 0 {
        return None;
    }
    let mut wide = vec![0u16; n as usize];
    let n = unsafe { MultiByteToWideChar(cp, 0, bytes.as_ptr(), len, wide.as_mut_ptr(), n) };
    (n > 0).then(|| String::from_utf16_lossy(&wide[..n as usize]))
}

#[cfg(not(windows))]
fn codepage(_: &[u8], _: bool) -> Option<String> {
    None
}

fn pump(mut from: impl Read + Send + 'static, out: Arc<Mutex<Output>>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = from.read(&mut buf) {
            if n == 0 {
                break;
            }
            out.lock().unwrap().push(&buf[..n]);
        }
    })
}

/// Выполнить команду в папке `dir`. `stop` спрашивается раз в секунду: «Стоп» в чате.
/// `tick` раз в пять секунд получает вывод на этот момент (фоновая команда несёт его сайту).
pub fn run(dir: &Path, command: &str, timeout: u64, stop: &dyn Fn() -> bool, tick: &dyn Fn(&str)) -> Result<Value, String> {
    let shell = detect();
    let _counted = Counted::new();
    let generation = GENERATION.load(Ordering::SeqCst);
    let mut cmd = Command::new(&shell.exe);
    if shell.name == "powershell" {
        let utf8 = "$OutputEncoding=[Console]::OutputEncoding=[Text.Encoding]::UTF8; ";
        cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command"]).arg(format!("{utf8}{command}"));
    } else {
        cmd.arg("-c").arg(command);
    }
    cmd.current_dir(dir)
        .env("GIT_PAGER", "cat").env("PAGER", "cat").env("NO_COLOR", "1").env("TERM", "dumb")
        .env("GIT_TERMINAL_PROMPT", "0").env("CHERE_INVOKING", "1")
        // Python в трубу пишет кодировкой Windows — пусть пишет UTF-8, как в терминале.
        .env("PYTHONIOENCODING", "utf-8").env("PYTHONUTF8", "1")
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let started = Instant::now();
    let mut child = cmd.spawn().map_err(|e| format!("cannot start {}: {e}", shell.name))?;
    let job = tree::Job::adopt(&child);
    let out = Arc::new(Mutex::new(Output::default()));
    let readers = [pump(child.stdout.take().unwrap(), out.clone()), pump(child.stderr.take().unwrap(), out.clone())];

    let limit = Duration::from_secs(timeout.clamp(1, MAX_SECS));
    let mut last_check = Instant::now();
    let mut last_tick = Instant::now();
    let mut killed: Option<&str> = None;
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break Some(s);
        }
        if started.elapsed() >= limit {
            killed = Some("timeout");
        } else if GENERATION.load(Ordering::SeqCst) != generation {
            killed = Some("stopped");
        } else if last_check.elapsed() >= Duration::from_secs(1) {
            last_check = Instant::now();
            if stop() {
                killed = Some("stopped");
            }
        }
        if killed.is_some() {
            job.kill();
            let _ = child.kill();
            break child.wait().ok();
        }
        if last_tick.elapsed() >= Duration::from_secs(5) {
            last_tick = Instant::now();
            let now = out.lock().unwrap().text();
            tick(&now);
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    // Потомки, что держат вывод открытым (фоновые процессы команды), — туда же. Но приложения
    // с окнами, что команда открыла (Start-Process winword, NX), остаются: их открыли, чтобы с ними работать.
    if killed.is_some() {
        job.kill();
    } else {
        job.release_apps();
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while readers.iter().any(|r| !r.is_finished()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    // Вывод всё ещё держит кто-то из оставленных — тогда уж всех.
    if readers.iter().any(|r| !r.is_finished()) {
        job.kill();
    }
    for r in readers {
        let _ = r.join();
    }

    let code = status.and_then(|s| s.code()).unwrap_or(-1);
    let mut text = out.lock().unwrap().text();
    match killed {
        Some("timeout") => text.push_str(&format!("\nCommand killed after the {}s timeout", limit.as_secs())),
        Some(_) => text.push_str("\nCommand stopped by the person"),
        None if code != 0 => text.push_str(&format!("\nCommand failed with exit code: {code}")),
        None => {}
    }
    Ok(json!({ "ok": true, "text": text.trim_start_matches('\n'),
        "meta": { "exit": code, "killed": killed.is_some(), "why": killed, "seconds": started.elapsed().as_secs() } }))
}

#[cfg(windows)]
mod tree {
    use std::collections::{HashMap, HashSet};
    use std::io::Read;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::JobObjects::*;
    use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE};

    /// Job Object с «убить всех при закрытии»: процесс и всё, что он запустит.
    pub struct Job(HANDLE);

    impl Job {
        pub fn adopt(child: &std::process::Child) -> Job {
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return Job(job);
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32);
                AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE);
                Job(job)
            }
        }

        pub fn kill(&self) {
            if !self.0.is_null() {
                unsafe { TerminateJobObject(self.0, 1) };
            }
        }

        /// Команда кончилась сама: оконные приложения (GUI-подсистема) и всё, что они запустили,
        /// оставить работать — снять с задания «убить при закрытии»; прочих (консольные фоновые) — убить.
        pub fn release_apps(&self) {
            if self.0.is_null() {
                return;
            }
            let pids = self.pids();
            let parents = parents();
            let gui: HashSet<u32> = pids.iter().copied().filter(|p| is_gui(*p)).collect();
            if gui.is_empty() {
                return self.kill();
            }
            let keep = |pid: u32| {
                let mut at = pid;
                for _ in 0..32 {
                    if gui.contains(&at) {
                        return true;
                    }
                    match parents.get(&at) {
                        Some(&up) if up != 0 && up != at => at = up,
                        _ => return false,
                    }
                }
                false
            };
            unsafe {
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = 0;
                SetInformationJobObject(self.0, JobObjectExtendedLimitInformation, &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32);
                for pid in pids.into_iter().filter(|p| !keep(*p)) {
                    let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
                    if !h.is_null() {
                        TerminateProcess(h, 1);
                        CloseHandle(h);
                    }
                }
            }
        }

        fn pids(&self) -> Vec<u32> {
            // JOBOBJECT_BASIC_PROCESS_ID_LIST: два счётчика u32 (вместе — одна ячейка usize), дальше номера по usize.
            let mut buf = vec![0usize; 1 + 1024];
            let ok = unsafe {
                QueryInformationJobObject(self.0, JobObjectBasicProcessIdList, buf.as_mut_ptr() as *mut _,
                    (buf.len() * std::mem::size_of::<usize>()) as u32, std::ptr::null_mut())
            };
            if ok == 0 {
                return Vec::new();
            }
            let n = ((buf[0] as u64 >> 32) as usize).min(1024);
            buf[1..1 + n].iter().map(|&p| p as u32).collect()
        }
    }

    /// Родитель каждого процесса (снимок Toolhelp).
    fn parents() -> HashMap<u32, u32> {
        let mut map = HashMap::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return map;
            }
            let mut e: PROCESSENTRY32W = std::mem::zeroed();
            e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snap, &mut e) != 0 {
                loop {
                    map.insert(e.th32ProcessID, e.th32ParentProcessID);
                    if Process32NextW(snap, &mut e) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
        }
        map
    }

    /// Оконная ли программа: подсистема в PE-заголовке её exe (2 — GUI). Консоль-хозяин — не в счёт.
    fn is_gui(pid: u32) -> bool {
        let Some(path) = image(pid) else { return false };
        let low = path.to_lowercase();
        if low.ends_with("conhost.exe") || low.ends_with("openconsole.exe") {
            return false;
        }
        let Ok(mut f) = std::fs::File::open(&path) else { return false };
        let mut head = [0u8; 1024];
        if f.read(&mut head).unwrap_or(0) < 0x40 {
            return false;
        }
        let pe = u32::from_le_bytes([head[0x3c], head[0x3d], head[0x3e], head[0x3f]]) as usize;
        if pe + 4 + 20 + 70 > head.len() || &head[pe..pe + 4] != b"PE\0\0" {
            return false;
        }
        let at = pe + 4 + 20 + 68;
        u16::from_le_bytes([head[at], head[at + 1]]) == 2
    }

    fn image(pid: u32) -> Option<String> {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return None;
            }
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
            CloseHandle(h);
            (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CloseHandle(self.0) };
            }
        }
    }
}

#[cfg(not(windows))]
mod tree {
    pub struct Job;

    impl Job {
        pub fn adopt(_: &std::process::Child) -> Job {
            Job
        }

        pub fn kill(&self) {}

        pub fn release_apps(&self) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // «Остановить всё» убивает и команды соседних тестов — команды тестов идут по одной.
    static ONE: Mutex<()> = Mutex::new(());

    fn one() -> std::sync::MutexGuard<'static, ()> {
        ONE.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn runs_reports_exit_codes_and_keeps_head_and_tail() {
        let _one = one();
        let dir = std::env::temp_dir();
        let ok = run(&dir, "echo hello", 20, &|| false, &|_| {}).unwrap();
        assert!(ok["text"].as_str().unwrap().contains("hello"));
        assert_eq!(ok["meta"]["exit"], 0);

        let bad = run(&dir, "exit 3", 20, &|| false, &|_| {}).unwrap();
        assert_eq!(bad["meta"]["exit"], 3);
        assert!(bad["text"].as_str().unwrap().contains("exit code: 3"));

        let mut o = Output::default();
        o.push(&vec![b'a'; HEAD + TAIL + 100]);
        let t = o.text();
        assert!(t.contains("100 bytes of output omitted"));
    }

    #[test]
    fn output_in_the_windows_code_page_is_read_right() {
        // «Привет, мир» в cp1251 и в UTF-8.
        let cp1251 = [0xCF, 0xF0, 0xE8, 0xE2, 0xE5, 0xF2, 0x2C, 0x20, 0xEC, 0xE8, 0xF0];
        if cfg!(windows) && unsafe { windows_sys::Win32::Globalization::GetACP() } == 1251 {
            assert_eq!(decode(&cp1251), "Привет, мир");
        }
        assert_eq!(decode("Привет, мир".as_bytes()), "Привет, мир");
        // Обрезанный с краю символ UTF-8 не переводит весь вывод в cp1251.
        let cut = &"Привет".as_bytes()[1..];
        assert_eq!(decode(cut), "\u{FFFD}ривет");
    }

    #[test]
    fn console_output_in_the_oem_code_page_is_read_right() {
        // «Привет, мир» в cp866 — так пишут PowerShell и cmd.
        let cp866 = [0x8F, 0xE0, 0xA8, 0xA2, 0xA5, 0xE2, 0x2C, 0x20, 0xAC, 0xA8, 0xE0];
        if cfg!(windows) && unsafe { windows_sys::Win32::Globalization::GetOEMCP() } == 866 {
            assert_eq!(decode(&cp866), "Привет, мир");
        }
    }

    #[test]
    fn stop_all_kills_running_commands_and_ticks_bring_output() {
        let _one = one();
        let dir = std::env::temp_dir();
        let long = if detect().name == "powershell" { "echo first; Start-Sleep 30" } else { "echo first; sleep 30" };
        let seen = Mutex::new(Vec::<String>::new());
        let h = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(7));
            stop_all();
        });
        let t0 = Instant::now();
        let r = run(&dir, long, 60, &|| false, &|o| seen.lock().unwrap().push(o.to_string())).unwrap();
        h.join().unwrap();
        assert!(t0.elapsed() < Duration::from_secs(12), "{:?}", t0.elapsed());
        assert_eq!(r["meta"]["why"], "stopped");
        assert!(seen.lock().unwrap().iter().any(|o| o.contains("first")));
        assert_eq!(running(), 0);
    }

    #[test]
    fn stop_kills_a_long_command() {
        let _one = one();
        let dir = std::env::temp_dir();
        let long = if detect().name == "powershell" { "Start-Sleep 30" } else { "sleep 30" };
        let t = Instant::now();
        let r = run(&dir, long, 60, &|| true, &|_| {}).unwrap();
        assert!(t.elapsed() < Duration::from_secs(10));
        assert_eq!(r["meta"]["killed"], true);

        // Оконное приложение, открытое командой, остаётся работать после неё (раньше умирало вместе с ней).
        #[cfg(windows)]
        {
            let start = if detect().name == "powershell" { "Start-Process notepad.exe -PassThru | ForEach-Object Id" }
                else { "powershell -NoProfile -Command 'Start-Process notepad.exe -PassThru | ForEach-Object Id'" };
            let r = run(&dir, start, 60, &|| false, &|_| {}).unwrap();
            let pid: u32 = r["text"].as_str().unwrap().trim().lines().last().unwrap().trim().parse().expect("pid of notepad");
            std::thread::sleep(Duration::from_millis(800));
            let alive = Command::new("tasklist").args(["/FI", &format!("PID eq {pid}")]).output().unwrap();
            let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/F"]).output();
            assert!(String::from_utf8_lossy(&alive.stdout).contains(&pid.to_string()), "notepad must survive the command");
        }

        // Фоновый потомок держит вывод открытым — Job Object убивает его вместе с командой.
        if detect().name == "bash" {
            let t = Instant::now();
            let r = run(&dir, "sleep 40 & echo started", 60, &|| false, &|_| {}).unwrap();
            assert!(t.elapsed() < Duration::from_secs(10), "команда не должна ждать фонового потомка");
            assert!(r["text"].as_str().unwrap().contains("started"));
        }
    }
}
