//! Event-driven WASAPI loopback on a dedicated COM thread. The guard joins it
//! before shutdown completes; the bounded channel never backpressures capture.
use std::{
    thread::JoinHandle,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{CloseHandle, HANDLE},
        Media::Audio::*,
        System::{
            Com::*,
            Threading::{CreateEventW, WaitForSingleObject},
        },
    },
};

pub struct Packet {
    pub at: Instant,
    pub pcm: Vec<u8>,
}
pub struct Capture {
    cancel: CancellationToken,
    thread: Option<JoinHandle<()>>,
}
impl Capture {
    pub fn start(cancel: CancellationToken) -> Result<(Self, mpsc::Receiver<Packet>), String> {
        let (send, receive) = mpsc::channel(8); // At most 800 ms; never replay on reconnect.
        let worker_cancel = cancel.clone();
        let thread = std::thread::Builder::new()
            .name("pulse-loopback".into())
            .spawn(move || {
                let _ = run(send, worker_cancel);
            })
            .map_err(|_| "Could not start Windows audio capture.")?;
        Ok((
            Self {
                cancel,
                thread: Some(thread),
            },
            receive,
        ))
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
struct Com;
impl Drop for Com {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
struct Event(HANDLE);
impl Drop for Event {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
struct Stream(IAudioClient);
impl Drop for Stream {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.Stop();
        }
    }
}

fn run(send: mpsc::Sender<Packet>, cancel: CancellationToken) -> windows::core::Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let _com = Com;
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        // A render endpoint with LOOPBACK captures system output, never a microphone.
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let format = WAVEFORMATEX {
            wFormatTag: 1,
            nChannels: 1,
            nSamplesPerSec: 24000,
            nAvgBytesPerSec: 48000,
            nBlockAlign: 2,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK
                | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY
                | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            1_000_000,
            0,
            &format,
            None,
        )?;
        let event = Event(CreateEventW(None, false, false, PCWSTR::null())?);
        client.SetEventHandle(event.0)?;
        let capture: IAudioCaptureClient = client.GetService()?;
        if cancel.is_cancelled() {
            return Ok(());
        }
        client.Start()?;
        let _stream = Stream(client);
        let mut pcm = Vec::with_capacity(4800);
        let mut at = Instant::now();
        while !cancel.is_cancelled() && !send.is_closed() {
            WaitForSingleObject(event.0, 20);
            while !cancel.is_cancelled() && capture.GetNextPacketSize()? > 0 {
                let mut data = std::ptr::null_mut();
                let mut frames = 0;
                let mut flags = 0;
                capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                if pcm.is_empty() {
                    at = Instant::now()
                        .checked_sub(Duration::from_secs_f64(frames as f64 / 24000.0))
                        .unwrap_or_else(Instant::now);
                }
                let bytes = frames as usize * 2;
                // WASAPI explicitly allows a null pointer for SILENT packets.
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    pcm.resize(pcm.len() + bytes, 0);
                } else {
                    pcm.extend_from_slice(std::slice::from_raw_parts(data, bytes));
                }
                capture.ReleaseBuffer(frames)?;
                if pcm.len() >= 4800 {
                    if cancel.is_cancelled() {
                        return Ok(());
                    }
                    let packet = Packet {
                        at,
                        pcm: std::mem::replace(&mut pcm, Vec::with_capacity(4800)),
                    };
                    if send.try_send(packet).is_err() {
                        return Ok(());
                    } // Reconnect on congestion; no stale backlog.
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires a Windows output device; captures locally without transmitting"]
    fn native_loopback_releases_device() {
        for _ in 0..3 {
            let cancel = CancellationToken::new();
            let (capture, mut receive) = Capture::start(cancel).unwrap();
            let end = Instant::now() + Duration::from_secs(2);
            let mut packets = 0;
            while Instant::now() < end {
                if let Ok(packet) = receive.try_recv() {
                    assert_eq!(packet.pcm.len() % 2, 0);
                    packets += 1;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let stop = Instant::now();
            drop(capture);
            assert!(stop.elapsed() < Duration::from_secs(1));
            assert!(
                packets > 0,
                "Play audio on the default output device for this test"
            );
        }
    }
}
