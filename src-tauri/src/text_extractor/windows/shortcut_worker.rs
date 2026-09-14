//! Private, inherited pipes carry shortcut settings and actions, never typed text.
use super::{
    hotkeys, session,
    shortcut_keys::{Action, Bindings},
};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Write},
    os::windows::process::CommandExt,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{channel, sync_channel, Sender},
        OnceLock,
    },
    time::Duration,
};

#[derive(Serialize, Deserialize)]
pub(crate) enum Control {
    Configure { enabled: bool, bindings: Bindings },
    Recording(bool),
    Resynchronize,
}

#[derive(PartialEq, Serialize, Deserialize)]
enum Event {
    Ready,
    Shortcut(Action),
}

const WORKER_ARGUMENT: &str = "--pulse-text-extractor-shortcuts";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
static CONTROLS: OnceLock<Sender<Option<Control>>> = OnceLock::new();
static AVAILABLE: AtomicBool = AtomicBool::new(false);

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(crate) fn run_if_requested() -> bool {
    if std::env::args_os().nth(1).as_deref() != Some(std::ffi::OsStr::new(WORKER_ARGUMENT)) {
        return false;
    }
    // This entry point runs before Tauri creates any webviews or tray icons.
    // WebView2 can bypass hooks hosted in its own process while focused.
    let _ = hotkeys::run_worker();
    true
}

pub(crate) fn available() -> bool {
    AVAILABLE.load(Ordering::Acquire)
}

pub(crate) fn send(control: Control) {
    if let Some(sender) = CONTROLS.get() {
        let _ = sender.send(Some(control));
    }
}

pub(crate) fn install(app: &tauri::AppHandle) -> Result<(), String> {
    if CONTROLS.get().is_some() {
        return Err("Text Extractor shortcuts are already installed.".into());
    }
    let executable = std::env::current_exe()
        .map_err(|_| "Could not locate Pulse for the Text Extractor shortcuts.")?;
    let mut worker = Worker(
        Command::new(executable)
            .arg(WORKER_ARGUMENT)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "Could not start the Text Extractor shortcut helper.")?,
    );
    let mut input = worker
        .0
        .stdin
        .take()
        .ok_or("Could not configure shortcut input.")?;
    let output = worker
        .0
        .stdout
        .take()
        .ok_or("Could not configure shortcut output.")?;
    let (controls, receive) = channel::<Option<Control>>();
    let (ready, started) = sync_channel(1);

    std::thread::Builder::new()
        .name("pulse-shortcut-controls".into())
        .spawn(move || {
            // Own the child here: failed startup or a closed control channel
            // drops this guard and reaps the process, including on timeout.
            let _worker = worker;
            while let Ok(Some(control)) = receive.recv() {
                if write_message(&mut input, &control).is_err() {
                    break;
                }
            }
        })
        .map_err(|_| "Could not configure the shortcut helper.")?;

    let stop = controls.clone();
    let app = app.clone();
    std::thread::Builder::new()
        .name("pulse-shortcut-actions".into())
        .spawn(move || {
            let mut messages = BufReader::new(output).lines();
            let initialized = messages
                .next()
                .and_then(Result::ok)
                .and_then(|line| serde_json::from_str::<Event>(&line).ok())
                == Some(Event::Ready);
            AVAILABLE.store(initialized, Ordering::Release);
            if ready.send(initialized).is_err() || !initialized {
                let _ = stop.send(None);
                AVAILABLE.store(false, Ordering::Release);
                return;
            }
            for line in messages {
                let event = line.ok().and_then(|line| serde_json::from_str(&line).ok());
                match event {
                    Some(Event::Shortcut(action)) => hotkeys::dispatch(&app, action),
                    _ => break,
                }
            }
            let _ = stop.send(None);
            AVAILABLE.store(false, Ordering::Release);
            session::shortcuts_stopped(&app);
        })
        .map_err(|_| "Could not receive Text Extractor shortcuts.")?;

    if !started
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or(false)
    {
        let _ = controls.send(None);
        return Err("Windows could not start the Text Extractor shortcuts.".into());
    }
    CONTROLS.set(controls).map_err(|sender| {
        let _ = sender.send(None);
        "Text Extractor shortcuts are already installed.".into()
    })
}

fn write_message(output: &mut impl Write, message: &impl Serialize) -> std::io::Result<()> {
    serde_json::to_writer(&mut *output, message)?;
    output.write_all(b"\n")?;
    output.flush()
}

pub(crate) fn ready() -> std::io::Result<()> {
    write_message(&mut std::io::stdout().lock(), &Event::Ready)
}

pub(crate) fn emit(action: Action) -> std::io::Result<()> {
    write_message(&mut std::io::stdout().lock(), &Event::Shortcut(action))
}

pub(crate) fn read_controls() {
    for line in std::io::stdin().lock().lines() {
        let control = line.ok().and_then(|line| serde_json::from_str(&line).ok());
        match control {
            Some(control) => hotkeys::apply(control),
            None => break,
        }
    }
    // Parent exit/crash closes its pipe. End the helper and release the hook
    // even while its main thread is waiting for keyboard input.
    std::process::exit(0);
}
