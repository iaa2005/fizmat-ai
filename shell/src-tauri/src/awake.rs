// «Не давать компьютеру гаснуть и блокироваться»: пока включено, Windows не гасит экран и не уходит
// в сон по бездействию — а значит, и не блокирует сеанс при отключении экрана (так компьютер и
// запирался, пока человек управлял им издалека). Так же держит экран PowerPoint во время показа.
// Win+L и блокировка по политике организации по-прежнему работают.
//
// Состояние SetThreadExecutionState принадлежит потоку — держит его свой поток, пока жива программа.

#![cfg(windows)]

use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED};

static KEEPER: OnceLock<Mutex<Sender<bool>>> = OnceLock::new();

fn keeper() -> &'static Mutex<Sender<bool>> {
    KEEPER.get_or_init(|| {
        let (tx, rx) = channel::<bool>();
        std::thread::spawn(move || {
            for on in rx {
                // SAFETY: только флаги, без указателей.
                unsafe {
                    SetThreadExecutionState(if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED } else { ES_CONTINUOUS });
                }
            }
        });
        Mutex::new(tx)
    })
}

pub fn set(on: bool) {
    let _ = keeper().lock().unwrap().send(on);
}
