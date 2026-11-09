// Команды оболочки для Fizmat Code (local_bash): Git Bash, если он есть, иначе
// PowerShell. Команда идёт в папке чата, без окна консоли, весь процесс с потомками —
// в Job Object: таймаут и «Стоп» в чате убивают дерево целиком, а по концу команды
// не остаются висеть забытые процессы. Вывод (stdout и stderr вместе) урезается с
// середины: голова и хвост — самое нужное модели.

use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const HEAD: usize = 12 * 1024;
const TAIL: usize = 36 * 1024;

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

/// Вывод в UTF-8 — как есть; иначе это кодировка Windows (cp1251 у Python и
/// многих программ в трубу). Обрезка головы и хвоста может разрезать один
/// символ UTF-8 — пара битых байтов на краях ещё не повод менять кодировку.
fn decode(bytes: &[u8]) -> String {
    let bad: usize = bytes.utf8_chunks().map(|c| c.invalid().len()).sum();
    if bad <= 4 {
        return String::from_utf8_lossy(bytes).to_string();
    }
    ansi(bytes).unwrap_or_else(|| String::from_utf8_lossy(bytes).to_string())
}

#[cfg(windows)]
fn ansi(bytes: &[u8]) -> Option<String> {
    use windows_sys::Win32::Globalization::{MultiByteToWideChar, CP_ACP};
    let len = i32::try_from(bytes.len()).ok()?;
    // SAFETY: буферы живые, длины переданы точно.
    let n = unsafe { MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), len, std::ptr::null_mut(), 0) };
    if n <= 0 {
        return None;
    }
    let mut wide = vec![0u16; n as usize];
    let n = unsafe { MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), len, wide.as_mut_ptr(), n) };
    (n > 0).then(|| String::from_utf16_lossy(&wide[..n as usize]))
}

#[cfg(not(windows))]
fn ansi(_: &[u8]) -> Option<String> {
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
pub fn run(dir: &Path, command: &str, timeout: u64, stop: &dyn Fn() -> bool) -> Result<Value, String> {
    let shell = detect();
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

    let limit = Duration::from_secs(timeout.clamp(1, 300));
    let mut last_check = Instant::now();
    let mut killed: Option<&str> = None;
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break Some(s);
        }
        if started.elapsed() >= limit {
            killed = Some("timeout");
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
        std::thread::sleep(Duration::from_millis(100));
    };
    // Потомки, что держат вывод открытым (фоновые процессы команды), — туда же.
    job.kill();
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
        "meta": { "exit": code, "killed": killed.is_some(), "seconds": started.elapsed().as_secs() } }))
}

#[cfg(windows)]
mod tree {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::*;

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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_reports_exit_codes_and_keeps_head_and_tail() {
        let dir = std::env::temp_dir();
        let ok = run(&dir, "echo hello", 20, &|| false).unwrap();
        assert!(ok["text"].as_str().unwrap().contains("hello"));
        assert_eq!(ok["meta"]["exit"], 0);

        let bad = run(&dir, "exit 3", 20, &|| false).unwrap();
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
    fn stop_kills_a_long_command() {
        let dir = std::env::temp_dir();
        let long = if detect().name == "powershell" { "Start-Sleep 30" } else { "sleep 30" };
        let t = Instant::now();
        let r = run(&dir, long, 60, &|| true).unwrap();
        assert!(t.elapsed() < Duration::from_secs(10));
        assert_eq!(r["meta"]["killed"], true);

        // Фоновый потомок держит вывод открытым — Job Object убивает его вместе с командой.
        if detect().name == "bash" {
            let t = Instant::now();
            let r = run(&dir, "sleep 40 & echo started", 60, &|| false).unwrap();
            assert!(t.elapsed() < Duration::from_secs(10), "команда не должна ждать фонового потомка");
            assert!(r["text"].as_str().unwrap().contains("started"));
        }
    }
}
