use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RadioEnd {
    Complete,
    Failed,
    Cancelled,
}
#[cfg(test)]
impl RadioEnd {
    pub(crate) fn is_ok(&self) -> bool {
        *self == RadioEnd::Complete
    }
}
pub(crate) struct Request {
    pub(crate) url: String,
    pub(crate) ffmpeg: PathBuf,
    pub(crate) pcm: crossbeam_channel::Sender<Vec<f32>>,
    pub(crate) done: std::sync::mpsc::Sender<RadioEnd>,
}

pub(crate) struct Slot {
    pub(crate) next: Option<Request>,
    pub(crate) version: u64,
    pub(crate) closed: bool,
}

type Shared = Arc<(Mutex<Slot>, Condvar)>;

pub(crate) struct RadioManager {
    shared: Shared,
    worker: Option<JoinHandle<()>>,
}

impl RadioManager {
    pub(crate) fn new() -> std::io::Result<Self> {
        let shared: Shared = Arc::new((
            Mutex::new(Slot {
                next: None,
                version: 0,
                closed: false,
            }),
            Condvar::new(),
        ));
        let shared_clone = shared.clone();
        let worker = thread::Builder::new().spawn(move || loop {
            let (req, version) = {
                let mut guard = shared_clone.0.lock().unwrap();
                while guard.next.is_none() && !guard.closed {
                    guard = shared_clone.1.wait(guard).unwrap();
                }
                match guard.next.take() {
                    Some(req) => (req, guard.version),
                    None => break,
                }
            };
            let result = run_radio(&req, &shared_clone, version);
            let guard = shared_clone.0.lock().unwrap();
            let end = if guard.closed || guard.version != version {
                RadioEnd::Cancelled
            } else if result.is_ok() {
                RadioEnd::Complete
            } else {
                RadioEnd::Failed
            };
            let _ = req.done.send(end);
        })?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }

    pub(crate) fn start(
        &self,
        url: &str,
        ffmpeg: PathBuf,
        pcm: crossbeam_channel::Sender<Vec<f32>>,
    ) -> Result<std::sync::mpsc::Receiver<RadioEnd>, ()> {
        if url.chars().any(|c| c.is_control()) {
            return Err(());
        }
        let parsed = tauri::Url::parse(url).map_err(|_| ())?;
        if parsed.scheme() != "http" && parsed.scheme() != "https" || parsed.host().is_none() {
            return Err(());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let mut guard = self.shared.0.lock().unwrap();
            if let Some(old) = guard.next.take() {
                let _ = old.done.send(RadioEnd::Cancelled);
            }
            guard.next = Some(Request {
                url: url.to_string(),
                ffmpeg,
                pcm,
                done: tx,
            });
            guard.version += 1;
        }
        self.shared.1.notify_all();
        Ok(rx)
    }

    pub(crate) fn cancel(&self) {
        let mut guard = self.shared.0.lock().unwrap();
        if let Some(old) = guard.next.take() {
            let _ = old.done.send(RadioEnd::Cancelled);
        }

        guard.version += 1;
        drop(guard);
        self.shared.1.notify_all();
    }
}

impl Drop for RadioManager {
    fn drop(&mut self) {
        let mut guard = self.shared.0.lock().unwrap();
        guard.closed = true;
        if let Some(old) = guard.next.take() {
            let _ = old.done.send(RadioEnd::Cancelled);
        }

        drop(guard);
        self.shared.1.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub(crate) fn obsolete(shared: &Shared, version: u64) -> bool {
    let guard = shared.0.lock().unwrap();
    guard.closed || guard.version != version
}

struct OwnedChild(std::process::Child);

impl std::ops::Drop for OwnedChild {
    fn drop(&mut self) {
        match self.0.try_wait() {
            Ok(std::option::Option::Some(_)) => return,
            _ => {}
        }
        let _ = self.0.kill();
        loop {
            match self.0.wait() {
                Ok(_) => return,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return,
            }
        }
    }
}

fn system_curl() -> std::io::Result<std::path::PathBuf> {
    let system_root = std::env::var_os("SystemRoot").ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "SystemRoot environment variable is not set",
        )
    })?;
    let path = std::path::PathBuf::from(system_root)
        .join("System32")
        .join("curl.exe");
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "SystemRoot is not an absolute path",
        ))
    }
}

fn spawn_radio(
    req: &Request,
) -> std::io::Result<(OwnedChild, OwnedChild, std::process::ChildStdout)> {
    let curl = std::process::Command::new(system_curl()?)
        .args([
            "-q",
            "--globoff",
            "--silent",
            "--fail",
            "--location",
            "--max-redirs",
            "5",
            "--connect-timeout",
            "10",
            "--proto",
            "=http,https",
            "--proto-redir",
            "=http,https",
            "--url",
            &req.url,
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    let mut curl = OwnedChild(curl);
    let out = curl
        .0
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "curl stdout"))?;
    let ffmpeg = std::process::Command::new(&req.ffmpeg)
        .args([
            "-nostdin",
            "-protocol_whitelist",
            "pipe",
            "-format_whitelist",
            "wav,flac,mp3,ogg,aac,mov,matroska,asf",
            "-i",
            "pipe:0",
            "-f",
            "s16le",
            "-ac",
            "2",
            "-ar",
            "44100",
            "pipe:1",
        ])
        .stdin(out)
        .stderr(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    let mut ffmpeg = OwnedChild(ffmpeg);
    let decoder = ffmpeg
        .0
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "ffmpeg stdout"))?;
    Ok((curl, ffmpeg, decoder))
}

use std::io::{self, Read};
use std::process::ChildStdout;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::{SendTimeoutError, Sender};
use std::time::Duration;

fn pump_radio(
    mut input: ChildStdout,
    tx: Sender<Vec<f32>>,
    cancel: Arc<AtomicBool>,
) -> Result<bool, ()> {
    const BUF_SIZE: usize = 8192;
    const SEND_TIMEOUT: Duration = Duration::from_millis(100);

    let mut carry: Option<u8> = None;
    let mut saw_audio = false;
    let mut buf = [0u8; BUF_SIZE];

    loop {
        if cancel.load(Ordering::Acquire) {
            return Ok(false);
        }

        let n = match input.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(()),
        };

        let mut bytes = Vec::with_capacity(n + 1);
        if let Some(c) = carry.take() {
            bytes.push(c);
        }
        bytes.extend_from_slice(&buf[..n]);

        let usable = bytes.len() & !1;
        if usable > 0 {
            let samples: Vec<f32> = bytes[..usable]
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
                .collect();

            if !samples.is_empty() {
                saw_audio = true;

                let mut unsent = samples;
                loop {
                    if cancel.load(Ordering::Acquire) {
                        return Ok(false);
                    }
                    match tx.send_timeout(unsent, SEND_TIMEOUT) {
                        Ok(()) => break,
                        Err(SendTimeoutError::Timeout(again)) => unsent = again,
                        Err(SendTimeoutError::Disconnected(_)) => return Err(()),
                    }
                }
            }
        }

        carry = if bytes.len() % 2 == 1 {
            Some(bytes[bytes.len() - 1])
        } else {
            None
        };
    }

    Ok(saw_audio)
}

fn run_radio(req: &Request, shared: &Shared, version: u64) -> Result<(), ()> {
    let (mut fetch, mut decode, out) = spawn_radio(req).map_err(|_| ())?;
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = cancel.clone();
    let tx = req.pcm.clone();
    let reader_failed = Arc::new(AtomicBool::new(false));
    let failed = reader_failed.clone();
    let reader = std::thread::Builder::new()
        .spawn(move || {
            let result = pump_radio(out, tx, flag);
            failed.store(result.is_err(), Ordering::Release);
            result
        })
        .map_err(|_| ())?;
    let result = watch_radio(
        &mut fetch,
        &mut decode,
        &reader,
        &reader_failed,
        shared,
        version,
    );
    cancel.store(true, std::sync::atomic::Ordering::Release);
    drop(fetch);
    drop(decode);
    let saw = reader.join();
    if obsolete(shared, version) {
        Ok(())
    } else if matches!(saw, Ok(Ok(true))) {
        result
    } else {
        Err(())
    }
}
fn watch_radio(
    fetch: &mut OwnedChild,
    decode: &mut OwnedChild,
    reader: &std::thread::JoinHandle<Result<bool, ()>>,
    reader_failed: &AtomicBool,
    shared: &Shared,
    version: u64,
) -> Result<(), ()> {
    loop {
        if obsolete(shared, version) {
            return Ok(());
        }
        if reader_failed.load(Ordering::Acquire) {
            return Err(());
        }
        let a = fetch.0.try_wait().map_err(|_| ())?;
        let b = decode.0.try_wait().map_err(|_| ())?;
        let f_done = a.is_some();
        let d_done = b.is_some();
        if f_done && !a.unwrap().success() {
            return Err(());
        }
        if d_done && !b.unwrap().success() {
            return Err(());
        }
        if f_done && d_done && reader.is_finished() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
#[cfg(all(test, windows))]
#[path = "radio_tests.rs"]
mod tests;
