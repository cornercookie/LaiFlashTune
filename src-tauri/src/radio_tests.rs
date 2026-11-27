fn wav_header(sample_count: u32) -> [u8; 44] {
    let byte_rate = 44100u32 * 2 * 2; // sample_rate * channels * bytes_per_sample
    let block_align = 4u16; // channels * bytes_per_sample
    let data_size = sample_count * 4;
    let chunk_size = 36 + data_size;
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&chunk_size.to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    h[22..24].copy_from_slice(&2u16.to_le_bytes()); // stereo
    h[24..28].copy_from_slice(&44100u32.to_le_bytes());
    h[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    h[32..34].copy_from_slice(&block_align.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes()); // bits per sample
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_size.to_le_bytes());
    h
}

/// Generates the known 100ms stereo 44100Hz s16 WAV: 4410 frames * 4 bytes = 17640 data bytes.
fn generate_test_wav() -> Vec<u8> {
    let sample_count = 4410u32; // 100ms * 44100
    let mut wav = wav_header(sample_count).to_vec();
    wav.resize(44 + sample_count as usize * 4, 0);
    wav
}

fn serve(hops: u32) -> (String, std::thread::JoinHandle<Vec<u32>>) {
    use std::io::{BufRead, Write};
    use std::net::{Shutdown, TcpListener};
    use std::time::{Duration, Instant};
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.set_nonblocking(true).unwrap();
    let u = format!(
        "http://127.0.0.1:{}/{}",
        l.local_addr().unwrap().port(),
        hops
    );
    let m = hops.min(5) + 1;
    let h = std::thread::spawn(move || {
        let s = Instant::now();
        let mut v = Vec::new();
        let mut a = 0u32;
        while a < m && s.elapsed() < Duration::from_secs(7) {
            match l.accept() {
                Ok((mut st, _)) => {
                    st.set_nonblocking(false).unwrap();
                    a += 1;
                    st.set_read_timeout(Some(Duration::from_secs(1))).ok();
                    let mut r = std::io::BufReader::new(&st);
                    let mut f = String::new();
                    if r.read_line(&mut f).is_ok() {
                        let mut line = String::new();
                        while r
                            .read_line(&mut line)
                            .map(|z| z > 0 && !line.trim().is_empty())
                            .unwrap_or(false)
                        {
                            line.clear();
                        }
                        if let Some(n) = f
                            .split_whitespace()
                            .nth(1)
                            .and_then(|p| p.trim_start_matches('/').parse::<u32>().ok())
                        {
                            v.push(n);
                            if n > 0 {
                                let _ = st.write_all(format!("HTTP/1.1 302 Found\r\nLocation: /{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", n - 1).as_bytes());
                            } else {
                                let w = generate_test_wav();
                                let _ = st.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", w.len()).as_bytes());
                                let _ = st.write_all(&w);
                            }
                        }
                    }
                    let _ = st.shutdown(Shutdown::Both);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                _ => break,
            }
        }
        v
    });
    (u, h)
}
#[test]
#[ignore]
fn radio_hops() {
    let manager = super::RadioManager::new().unwrap();
    let ff = crate::tools::locate("ffmpeg.exe").unwrap().0;
    for hops in [0u32, 5, 6] {
        let (url, server) = serve(hops);
        let (tx, pcm) = crossbeam_channel::bounded(128);
        let terminal = manager.start(&url, ff.clone(), tx).unwrap();
        let msg = terminal
            .recv_timeout(std::time::Duration::from_secs(8))
            .unwrap();
        assert_eq!(msg.is_ok(), hops <= 5);
        assert_eq!(
            pcm.try_iter().map(|batch| batch.len()).sum::<usize>(),
            if hops <= 5 { 8820 } else { 0 }
        );
        assert_eq!(
            server.join().unwrap(),
            (hops.saturating_sub(5)..=hops).rev().collect::<Vec<_>>()
        );
    }
}

#[test]
#[ignore]
fn radio_cancel_stalled() {
    let manager = super::RadioManager::new().unwrap();
    let ff = crate::tools::locate("ffmpeg.exe").unwrap().0;
    let (tx, pcm) = crossbeam_channel::bounded(128);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let terminal = manager.start(&url, ff.clone(), tx).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut socket = None;
    while std::time::Instant::now() < deadline {
        match listener.accept() {
            Ok((s, _addr)) => {
                socket = Some(s);
                break;
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(e) => panic!("accept error: {}", e),
        }
    }
    let socket = socket.expect("stalled client did not connect within 5s");
    manager.cancel();
    drop(pcm);
    assert_eq!(
        terminal
            .recv_timeout(std::time::Duration::from_secs(8))
            .unwrap(),
        super::RadioEnd::Cancelled
    );
    let (url2, h) = serve(0);
    let (tx2, pcm2) = crossbeam_channel::bounded(128);
    let terminal2 = manager.start(&url2, ff.clone(), tx2).unwrap();
    let mut total = 0usize;
    while total < 8820 {
        let chunk = pcm2
            .recv_timeout(std::time::Duration::from_secs(8))
            .unwrap();
        total += chunk.iter().take(8820 - total).count();
    }
    assert_eq!(total, 8820);
    assert_eq!(
        terminal2
            .recv_timeout(std::time::Duration::from_secs(8))
            .unwrap(),
        super::RadioEnd::Complete
    );
    assert_eq!(h.join().unwrap(), vec![0]);
    drop(socket);
}
#[cfg(windows)]
#[test]
#[ignore]
fn radio_children_reaped() {
    use std::os::windows::io::{AsHandle, AsRawHandle};

    #[link(name = "kernel32")]
    extern "system" {
        fn WaitForSingleObject(handle: *mut std::ffi::c_void, milliseconds: u32) -> u32;
    }

    const WAIT_OBJECT_0: u32 = 0;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    let (pcm_tx, _pcm_rx) = crossbeam_channel::unbounded();
    let (done_tx, _done_rx) = std::sync::mpsc::channel();

    let req = super::Request {
        url: format!("http://127.0.0.1:{port}"),
        ffmpeg: crate::tools::locate("ffmpeg.exe").unwrap().0,
        pcm: pcm_tx,
        done: done_tx,
    };

    let (child_a, child_b, stdout) = super::spawn_radio(&req).unwrap();

    let handle_a = child_a.0.as_handle().try_clone_to_owned().unwrap();
    let handle_b = child_b.0.as_handle().try_clone_to_owned().unwrap();

    drop(child_a);
    drop(child_b);
    drop(stdout);

    assert_eq!(
        unsafe { WaitForSingleObject(handle_a.as_raw_handle() as *mut std::ffi::c_void, 3000) },
        WAIT_OBJECT_0
    );
    assert_eq!(
        unsafe { WaitForSingleObject(handle_b.as_raw_handle() as *mut std::ffi::c_void, 3000) },
        WAIT_OBJECT_0
    );
}
#[test]
#[ignore]
fn radio_backpressure_and_drop() {
    use std::time::{Duration, Instant};
    let limit = Duration::from_secs(5);
    let manager = super::RadioManager::new().unwrap();
    let ff = crate::tools::locate("ffmpeg.exe").unwrap().0;
    let (url, h) = serve(0);
    let (tx, pcm) = crossbeam_channel::bounded(1);
    let terminal = manager.start(&url, ff, tx).unwrap();
    let started = Instant::now();
    while !pcm.is_full() && started.elapsed() < limit {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(pcm.is_full());
    let (done, wait) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        drop(manager);
        done.send(()).unwrap();
    });
    wait.recv_timeout(limit).unwrap();
    assert_eq!(
        terminal.recv_timeout(limit).unwrap(),
        super::RadioEnd::Cancelled
    );
    assert_eq!(h.join().unwrap(), vec![0]);
    assert!(pcm.is_full());
}
#[test]
#[ignore]
fn radio_failed_decoder_start_and_retry() {
    let m = super::RadioManager::new().unwrap();
    let (tx, _pcm) = crossbeam_channel::bounded(1);
    let limit = std::time::Duration::from_secs(8);
    let fail = m
        .start(
            "http://127.0.0.1:1/test",
            std::path::PathBuf::from("Z:/missing-test-decoder.exe"),
            tx,
        )
        .unwrap();
    assert_eq!(fail.recv_timeout(limit).unwrap(), super::RadioEnd::Failed);
    let (url, h) = serve(0);
    let (tx, pcm) = crossbeam_channel::bounded(128);
    let ff = crate::tools::locate("ffmpeg.exe").unwrap().0;
    let ok = m.start(&url, ff, tx).unwrap();
    assert_eq!(ok.recv_timeout(limit).unwrap(), super::RadioEnd::Complete);
    assert_eq!(pcm.try_iter().map(|v| v.len()).sum::<usize>(), 8820);
    assert_eq!(h.join().unwrap(), vec![0]);
}
#[test]
#[ignore]
fn radio_http_failure_and_empty_response() {
    use std::io::{BufRead, Write};
    use std::time::{Duration, Instant};
    let manager = super::RadioManager::new().unwrap();
    let decoder = crate::tools::locate("ffmpeg.exe").unwrap().0;
    for status in ["404 Not Found", "200 OK"] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/station", listener.local_addr().unwrap());
        let (tx, pcm) = crossbeam_channel::bounded(128);
        let done = manager.start(&url, decoder.clone(), tx).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut socket = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(e) => panic!("radio connection: {e}"),
            }
        };
        socket.set_nonblocking(false).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = std::io::BufReader::new(&socket);
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                break;
            }
        }
        write!(
            socket,
            "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        drop(socket);
        assert_eq!(
            done.recv_timeout(Duration::from_secs(5)).unwrap(),
            super::RadioEnd::Failed
        );
        assert!(pcm.try_iter().next().is_none());
    }
}
