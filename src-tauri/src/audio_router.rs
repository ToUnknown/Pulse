use std::{
    fs,
    io::{self, ErrorKind},
    net::UdpSocket,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

use tauri::AppHandle;
#[cfg(target_os = "macos")]
use tauri::Manager;

use crate::translation::{build_passthrough_task, PassthroughTask, TranslationConfig};

const ROUTER_ADDRESS: &str = "127.0.0.1:41874";
const ROUTER_ARGUMENT: &str = "--audio-router";
const ROUTER_STOP_ARGUMENT: &str = "--stop-audio-router";
const ROUTER_PONG: &str = "PONG 1";
const ROUTER_LEASE: Duration = Duration::from_millis(1_500);
const ROUTER_HEARTBEAT: Duration = Duration::from_millis(400);
const ROUTER_POLL: Duration = Duration::from_millis(50);
const ROUTER_RETRY: Duration = Duration::from_secs(1);
const DEVICE_REFRESH: Duration = Duration::from_secs(1);
const STARTUP_GRACE: Duration = Duration::from_millis(1_000);
const CONSUMER_REFRESH: Duration = Duration::from_millis(50);
const CONSUMER_RELEASE_DELAY: Duration = Duration::from_millis(500);

#[cfg(target_os = "macos")]
const MACOS_LAUNCH_AGENT_LABEL: &str = "app.pulse.desktop.audio-router";
#[cfg(target_os = "windows")]
const WINDOWS_RUN_VALUE: &str = "Pulse Audio Router";

#[derive(Clone)]
pub(crate) struct AudioRouterController {
    config_path: PathBuf,
}

impl AudioRouterController {
    pub(crate) fn new(config_path: PathBuf) -> Self {
        Self { config_path }
    }

    pub(crate) fn enable(&self, app: &AppHandle) -> Result<(), String> {
        register_at_login(app, &self.config_path)?;
        self.ensure_running()?;
        self.resume()
    }

    pub(crate) fn disable(&self, app: &AppHandle) -> Result<(), String> {
        unregister_at_login(app)?;
        self.shutdown()
    }

    pub(crate) fn reload(&self) -> Result<(), String> {
        self.ensure_running()?;
        request("RELOAD").map(|_| ())
    }

    pub(crate) fn pause(&self) -> Result<AudioRouterLease, String> {
        self.ensure_running()?;
        request("PAUSE")?;
        AudioRouterLease::new(self.clone())
    }

    pub(crate) fn error(&self) -> Option<String> {
        match request("STATUS") {
            Ok(status) if status == "OK" || status == "PAUSED" => None,
            Ok(status) => status.strip_prefix("ERROR ").map(str::to_string),
            Err(error) => Some(error),
        }
    }

    pub(crate) fn stop_for_update(&self, _app: &AppHandle) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        stop_macos_launch_agent(_app)?;
        self.shutdown()
    }

    pub(crate) fn ensure_running(&self) -> Result<(), String> {
        if request("PING").as_deref() == Ok(ROUTER_PONG) {
            return Ok(());
        }
        let _ = request("SHUTDOWN");

        spawn_router(&self.config_path)?;
        for _ in 0..30 {
            thread::sleep(Duration::from_millis(100));
            if request("PING").as_deref() == Ok(ROUTER_PONG) {
                return Ok(());
            }
        }
        Err("the Pulse audio router did not start".to_string())
    }

    fn resume(&self) -> Result<(), String> {
        request("RESUME").map(|_| ())
    }

    fn shutdown(&self) -> Result<(), String> {
        match request("SHUTDOWN") {
            Ok(_) => {
                for _ in 0..20 {
                    thread::sleep(Duration::from_millis(50));
                    if request("PING").is_err() {
                        return Ok(());
                    }
                }
                Err("the Pulse audio router did not stop".to_string())
            }
            Err(_) => Ok(()),
        }
    }
}

pub(crate) struct AudioRouterLease {
    controller: AudioRouterController,
    stop: Arc<AtomicBool>,
    heartbeat: Option<thread::JoinHandle<()>>,
}

impl AudioRouterLease {
    fn new(controller: AudioRouterController) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let heartbeat_stop = stop.clone();
        let heartbeat = thread::Builder::new()
            .name("pulse-audio-router-lease".to_string())
            .spawn(move || {
                while !heartbeat_stop.load(Ordering::Acquire) {
                    thread::park_timeout(ROUTER_HEARTBEAT);
                    if !heartbeat_stop.load(Ordering::Acquire) {
                        let _ = request("PAUSE");
                    }
                }
            })
            .map_err(|error| format!("could not monitor the Pulse audio router: {error}"))?;

        Ok(Self {
            controller,
            stop,
            heartbeat: Some(heartbeat),
        })
    }
}

impl Drop for AudioRouterLease {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(heartbeat) = self.heartbeat.take() {
            heartbeat.thread().unpark();
            let _ = heartbeat.join();
        }
        let _ = self.controller.resume();
    }
}

pub fn run_if_requested() -> bool {
    let mut arguments = std::env::args_os();
    let _ = arguments.next();
    let argument = arguments.next();
    if argument.as_deref() == Some(std::ffi::OsStr::new(ROUTER_STOP_ARGUMENT)) {
        let _ = request("SHUTDOWN");
        return true;
    }
    if argument.as_deref() != Some(std::ffi::OsStr::new(ROUTER_ARGUMENT)) {
        return false;
    }

    let Some(config_path) = arguments.next().map(PathBuf::from) else {
        eprintln!("Pulse audio router needs its configuration path");
        return true;
    };
    if let Err(error) = run_router(config_path) {
        eprintln!("Pulse audio router stopped: {error}");
    }
    true
}

fn run_router(config_path: PathBuf) -> Result<(), String> {
    let socket = match UdpSocket::bind(ROUTER_ADDRESS) {
        Ok(socket) => socket,
        Err(error) if error.kind() == ErrorKind::AddrInUse => return Ok(()),
        Err(error) => return Err(format!("could not open its control channel: {error}")),
    };
    socket
        .set_read_timeout(Some(ROUTER_POLL))
        .map_err(|error| error.to_string())?;

    let mut task: Option<PassthroughTask> = None;
    let mut active_device = None::<String>;
    let mut last_error = None::<String>;
    let mut paused_until = Instant::now() + STARTUP_GRACE;
    let mut consumer_active = false;
    let mut consumer_detected = false;
    let mut consumer_release_at = Instant::now();
    #[cfg(target_os = "macos")]
    let mut consumer_monitor = MacosPulseConsumerMonitor::new();
    #[cfg(target_os = "windows")]
    let mut consumer_monitor = WindowsPulseConsumerMonitor::new();
    let mut next_consumer_refresh = Instant::now();
    let mut last_config_modified = None::<SystemTime>;
    let mut next_device_refresh = Instant::now();
    let mut retry_at = Instant::now();
    let mut force_reload = true;
    let mut buffer = [0_u8; 512];

    loop {
        match socket.recv_from(&mut buffer) {
            Ok((length, sender)) => {
                let command = std::str::from_utf8(&buffer[..length]).unwrap_or_default();
                let response = match command {
                    "PING" => ROUTER_PONG.to_string(),
                    "PAUSE" => {
                        paused_until = Instant::now() + ROUTER_LEASE;
                        task.take();
                        active_device = None;
                        "OK".to_string()
                    }
                    "RESUME" => {
                        paused_until = Instant::now();
                        force_reload = true;
                        "OK".to_string()
                    }
                    "RELOAD" => {
                        task.take();
                        active_device = None;
                        force_reload = true;
                        "OK".to_string()
                    }
                    "STATUS" => {
                        if Instant::now() < paused_until {
                            "PAUSED".to_string()
                        } else if let Some(error) = &last_error {
                            format!("ERROR {error}")
                        } else {
                            "OK".to_string()
                        }
                    }
                    "SHUTDOWN" => {
                        task.take();
                        let _ = socket.send_to(b"OK", sender);
                        return Ok(());
                    }
                    _ => "ERROR unsupported command".to_string(),
                };
                let _ = socket.send_to(response.as_bytes(), sender);
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) => return Err(format!("control channel failed: {error}")),
        }

        let now = Instant::now();
        if now < paused_until {
            continue;
        }

        if now >= next_consumer_refresh {
            next_consumer_refresh = now + CONSUMER_REFRESH;
            match consumer_monitor.is_active() {
                Ok(active) => {
                    consumer_detected = active;
                    if !active && task.is_none() {
                        last_error = None;
                    }
                }
                Err(error) => {
                    last_error = Some(error);
                    consumer_detected = false;
                }
            }
        }

        if consumer_detected {
            if !consumer_active {
                force_reload = true;
            }
            consumer_active = true;
            consumer_release_at = now + CONSUMER_RELEASE_DELAY;
        } else if consumer_active && now >= consumer_release_at {
            consumer_active = false;
            task.take();
            active_device = None;
        }
        if !consumer_active {
            continue;
        }

        let modified = fs::metadata(&config_path)
            .and_then(|metadata| metadata.modified())
            .ok();
        if modified != last_config_modified {
            last_config_modified = modified;
            force_reload = true;
        }

        if let Some(error) = task.as_ref().and_then(PassthroughTask::error) {
            last_error = Some(error);
            task.take();
            active_device = None;
            retry_at = now + ROUTER_RETRY;
        }

        if !force_reload && now < next_device_refresh {
            continue;
        }
        next_device_refresh = now + DEVICE_REFRESH;

        let config = TranslationConfig::load(&config_path);
        if !config.enabled {
            task.take();
            active_device = None;
            last_error = None;
            force_reload = false;
            continue;
        }

        let desired_device = crate::translation::router_input_device_name(&config);
        if desired_device
            .as_ref()
            .is_ok_and(|name| active_device.as_ref() == Some(name))
            && task.is_some()
            && !force_reload
        {
            continue;
        }
        if now < retry_at && !force_reload {
            continue;
        }

        task.take();
        active_device = None;
        match build_passthrough_task(&config) {
            Ok((next_task, device_name)) => {
                task = Some(next_task);
                active_device = Some(device_name);
                last_error = None;
                retry_at = now;
            }
            Err(error) => {
                last_error = Some(error);
                retry_at = now + ROUTER_RETRY;
            }
        }
        force_reload = false;
    }
}

fn request(command: &str) -> Result<String, String> {
    let socket = UdpSocket::bind("127.0.0.1:0")
        .map_err(|error| format!("could not contact the Pulse audio router: {error}"))?;
    socket
        .set_read_timeout(Some(Duration::from_millis(250)))
        .map_err(|error| error.to_string())?;
    socket
        .send_to(command.as_bytes(), ROUTER_ADDRESS)
        .map_err(|error| format!("could not contact the Pulse audio router: {error}"))?;

    let mut response = [0_u8; 512];
    let (length, _) = socket
        .recv_from(&mut response)
        .map_err(|error| format!("the Pulse audio router did not respond: {error}"))?;
    String::from_utf8(response[..length].to_vec())
        .map_err(|error| format!("the Pulse audio router returned invalid data: {error}"))
}

fn spawn_router(config_path: &Path) -> Result<(), String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not locate the Pulse audio router: {error}"))?;
    let mut command = Command::new(executable);
    command
        .arg(ROUTER_ARGUMENT)
        .arg(config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, DETACHED_PROCESS};
        command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }

    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("could not launch the Pulse audio router: {error}"))
}

#[cfg(target_os = "macos")]
fn register_at_login(app: &AppHandle, config_path: &Path) -> Result<(), String> {
    if cfg!(debug_assertions) {
        return Ok(());
    }

    let launch_agent = macos_launch_agent_path(app)?;
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not locate the Pulse audio router: {error}"))?;
    let contents = macos_launch_agent_contents(&executable, config_path);
    let current = fs::read_to_string(&launch_agent).ok();
    if current.as_deref() != Some(&contents) {
        let _ = stop_macos_launch_agent(app);
        if let Some(parent) = launch_agent.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(&launch_agent, contents)
            .map_err(|error| format!("could not register the Pulse audio router: {error}"))?;
    }

    let domain = macos_launch_domain()?;
    let service = format!("{domain}/{MACOS_LAUNCH_AGENT_LABEL}");
    if Command::new("/bin/launchctl")
        .args(["print", &service])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
    {
        return Ok(());
    }

    let status = Command::new("/bin/launchctl")
        .arg("bootstrap")
        .arg(domain)
        .arg(&launch_agent)
        .status()
        .map_err(|error| format!("could not register the Pulse audio router: {error}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| "macOS could not register the Pulse audio router".to_string())
}

#[cfg(target_os = "macos")]
fn unregister_at_login(app: &AppHandle) -> Result<(), String> {
    if cfg!(debug_assertions) {
        return Ok(());
    }
    stop_macos_launch_agent(app)?;
    match fs::remove_file(macos_launch_agent_path(app)?) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "could not unregister the Pulse audio router: {error}"
        )),
    }
}

#[cfg(target_os = "macos")]
fn stop_macos_launch_agent(_app: &AppHandle) -> Result<(), String> {
    if cfg!(debug_assertions) {
        return Ok(());
    }
    let service = format!("{}/{MACOS_LAUNCH_AGENT_LABEL}", macos_launch_domain()?);
    let status = Command::new("/bin/launchctl")
        .args(["bootout", &service])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("could not stop the Pulse audio router: {error}"))?;
    if status.success() || request("PING").is_err() {
        Ok(())
    } else {
        Err("macOS could not stop the Pulse audio router".to_string())
    }
}

#[cfg(target_os = "macos")]
fn macos_launch_agent_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .home_dir()
        .map(|home| {
            home.join("Library/LaunchAgents")
                .join(format!("{MACOS_LAUNCH_AGENT_LABEL}.plist"))
        })
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
fn macos_launch_domain() -> Result<String, String> {
    let output = Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .map_err(|error| format!("could not identify the current macOS user: {error}"))?;
    let user_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || user_id.is_empty() {
        return Err("could not identify the current macOS user".to_string());
    }
    Ok(format!("gui/{user_id}"))
}

#[cfg(target_os = "macos")]
fn macos_launch_agent_contents(executable: &Path, config_path: &Path) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
  <key>Label</key>\n\
  <string>{MACOS_LAUNCH_AGENT_LABEL}</string>\n\
  <key>ProgramArguments</key>\n\
  <array>\n\
    <string>{}</string>\n\
    <string>{ROUTER_ARGUMENT}</string>\n\
    <string>{}</string>\n\
  </array>\n\
  <key>RunAtLoad</key>\n\
  <true/>\n\
  <key>KeepAlive</key>\n\
  <true/>\n\
  <key>ProcessType</key>\n\
  <string>Background</string>\n\
</dict>\n\
</plist>\n",
        xml_escape(&executable.to_string_lossy()),
        xml_escape(&config_path.to_string_lossy()),
    )
}

#[cfg(target_os = "macos")]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(target_os = "windows")]
fn register_at_login(_app: &AppHandle, config_path: &Path) -> Result<(), String> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};

    if cfg!(debug_assertions) {
        return Ok(());
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not locate the Pulse audio router: {error}"))?;
    let command = format!(
        "\"{}\" {ROUTER_ARGUMENT} \"{}\"",
        executable.display(),
        config_path.display()
    );
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let (run, _) = current_user
        .create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
        .map_err(|error| format!("could not register the Pulse audio router: {error}"))?;
    run.set_value(WINDOWS_RUN_VALUE, &command)
        .map_err(|error| format!("could not register the Pulse audio router: {error}"))
}

#[cfg(target_os = "windows")]
fn unregister_at_login(_app: &AppHandle) -> Result<(), String> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};

    if cfg!(debug_assertions) {
        return Ok(());
    }
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let run = match current_user.open_subkey_with_flags(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
        winreg::enums::KEY_SET_VALUE,
    ) {
        Ok(run) => run,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "could not unregister the Pulse audio router: {error}"
            ))
        }
    };
    match run.delete_value(WINDOWS_RUN_VALUE) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "could not unregister the Pulse audio router: {error}"
        )),
    }
}

#[cfg(target_os = "macos")]
struct MacosPulseConsumerMonitor {
    device_id: Option<u32>,
}

#[cfg(target_os = "macos")]
impl MacosPulseConsumerMonitor {
    fn new() -> Self {
        Self { device_id: None }
    }

    fn is_active(&mut self) -> Result<bool, String> {
        let device_id = match self.device_id {
            Some(device_id) => device_id,
            None => {
                let device_id = find_macos_pulse_device()?;
                self.device_id = Some(device_id);
                device_id
            }
        };
        match macos_device_is_running(device_id) {
            Ok(active) => Ok(active),
            Err(error) => {
                self.device_id = None;
                Err(error)
            }
        }
    }
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct AudioObjectPropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[cfg(target_os = "macos")]
#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectGetPropertyData(
        object_id: u32,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier_data: *const std::ffi::c_void,
        data_size: *mut u32,
        data: *mut std::ffi::c_void,
    ) -> i32;
}

#[cfg(target_os = "macos")]
fn find_macos_pulse_device() -> Result<u32, String> {
    const PULSE_DEVICE_UID: &str = "app.pulse.desktop.virtual-microphone";
    coreaudio::audio_unit::macos_helpers::get_audio_device_ids()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|device_id| macos_device_uid(*device_id).as_deref() == Ok(PULSE_DEVICE_UID))
        .ok_or_else(|| "the Pulse virtual microphone is unavailable".to_string())
}

#[cfg(target_os = "macos")]
fn macos_device_uid(device_id: u32) -> Result<String, String> {
    use core_foundation::{base::TCFType, string::CFString};

    const DEVICE_UID: u32 = u32::from_be_bytes(*b"uid ");
    const GLOBAL_SCOPE: u32 = u32::from_be_bytes(*b"glob");
    let address = AudioObjectPropertyAddress {
        selector: DEVICE_UID,
        scope: GLOBAL_SCOPE,
        element: 0,
    };
    let mut value: core_foundation::string::CFStringRef = std::ptr::null();
    let mut size = std::mem::size_of::<core_foundation::string::CFStringRef>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            device_id,
            &address,
            0,
            std::ptr::null(),
            &mut size,
            (&mut value as *mut core_foundation::string::CFStringRef).cast(),
        )
    };
    if status != 0 || value.is_null() {
        return Err(format!(
            "could not read an audio-device identifier ({status})"
        ));
    }
    Ok(unsafe { CFString::wrap_under_create_rule(value) }.to_string())
}

#[cfg(target_os = "macos")]
fn macos_device_is_running(device_id: u32) -> Result<bool, String> {
    const DEVICE_IS_RUNNING: u32 = u32::from_be_bytes(*b"goin");
    const GLOBAL_SCOPE: u32 = u32::from_be_bytes(*b"glob");
    let address = AudioObjectPropertyAddress {
        selector: DEVICE_IS_RUNNING,
        scope: GLOBAL_SCOPE,
        element: 0,
    };
    let mut running = 0_u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            device_id,
            &address,
            0,
            std::ptr::null(),
            &mut size,
            (&mut running as *mut u32).cast(),
        )
    };
    if status == 0 {
        Ok(running != 0)
    } else {
        Err(format!(
            "could not inspect Pulse microphone consumers ({status})"
        ))
    }
}

#[cfg(target_os = "windows")]
struct WindowsPulseConsumerMonitor {
    session_manager: Option<windows::Win32::Media::Audio::IAudioSessionManager2>,
}

#[cfg(target_os = "windows")]
impl WindowsPulseConsumerMonitor {
    fn new() -> Self {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            );
        }
        Self {
            session_manager: None,
        }
    }

    fn is_active(&mut self) -> Result<bool, String> {
        if self.session_manager.is_none() {
            self.session_manager = Some(find_windows_pulse_session_manager()?);
        }
        let Some(manager) = self.session_manager.as_ref() else {
            return Ok(false);
        };

        let result = unsafe {
            let sessions = manager
                .GetSessionEnumerator()
                .map_err(|error| error.to_string())?;
            for index in 0..sessions.GetCount().map_err(|error| error.to_string())? {
                if sessions
                    .GetSession(index)
                    .and_then(|session| session.GetState())
                    .map_err(|error| error.to_string())?
                    == windows::Win32::Media::Audio::AudioSessionStateActive
                {
                    return Ok(true);
                }
            }
            Ok(false)
        };
        match result {
            Ok(active) => Ok(active),
            Err(error) => {
                self.session_manager = None;
                Err(format!(
                    "could not inspect Pulse microphone consumers: {error}"
                ))
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn find_windows_pulse_session_manager(
) -> Result<windows::Win32::Media::Audio::IAudioSessionManager2, String> {
    use windows::Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Media::Audio::{
            eCapture, IAudioSessionManager2, IMMDeviceEnumerator, MMDeviceEnumerator,
            DEVICE_STATE_ACTIVE,
        },
        System::Com::{
            CoCreateInstance, StructuredStorage::PropVariantToString, CLSCTX_ALL, STGM_READ,
        },
    };

    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|error| format!("could not inspect Windows microphones: {error}"))?;
        let devices = enumerator
            .EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)
            .map_err(|error| format!("could not inspect Windows microphones: {error}"))?;
        for index in 0..devices
            .GetCount()
            .map_err(|error| format!("could not inspect Windows microphones: {error}"))?
        {
            let device = devices
                .Item(index)
                .map_err(|error| format!("could not inspect Windows microphones: {error}"))?;
            let properties = device
                .OpenPropertyStore(STGM_READ)
                .map_err(|error| format!("could not inspect Windows microphones: {error}"))?;
            let value = properties
                .GetValue(&PKEY_Device_FriendlyName)
                .map_err(|error| format!("could not inspect Windows microphones: {error}"))?;
            let mut name = [0_u16; 512];
            PropVariantToString(&value, &mut name)
                .map_err(|error| format!("could not inspect Windows microphones: {error}"))?;
            let length = name
                .iter()
                .position(|character| *character == 0)
                .unwrap_or(name.len());
            if String::from_utf16_lossy(&name[..length]).eq_ignore_ascii_case("Pulse") {
                return device
                    .Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)
                    .map_err(|error| format!("could not monitor the Pulse microphone: {error}"));
            }
        }
    }
    Err("the Pulse virtual microphone is unavailable".to_string())
}
