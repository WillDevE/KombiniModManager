//! Tests against a tiny local HTTP server that supports `Range`.

use super::*;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;

struct Server {
    url: String,
    hits: Arc<AtomicUsize>,
}

/// Serves `body` at any path. When `cut_first` is set, the first full (non
/// range) response is cut off halfway to exercise resume. `delay` slows
/// every chunk so pause/cancel can be observed.
fn serve(body: Vec<u8>, cut_first: bool, delay: Duration) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/file.zip", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let cut = Arc::new(AtomicBool::new(cut_first));
    let body = Arc::new(body);
    let h = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let body = body.clone();
            let cut = cut.clone();
            h.fetch_add(1, Ordering::SeqCst);
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut range_start: Option<usize> = None;
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    if line == "\r\n" {
                        break;
                    }
                    let lower = line.to_ascii_lowercase();
                    if let Some(r) = lower.strip_prefix("range: bytes=") {
                        range_start = r.trim().trim_end_matches('-').parse().ok();
                    }
                    line.clear();
                }
                let start = range_start.unwrap_or(0).min(body.len());
                let slice = &body[start..];
                let head = if range_start.is_some() {
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                        slice.len(),
                        start,
                        body.len() - 1,
                        body.len()
                    )
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        slice.len()
                    )
                };
                let _ = stream.write_all(head.as_bytes());
                let send = if range_start.is_none() && cut.swap(false, Ordering::SeqCst) {
                    &slice[..slice.len() / 2]
                } else {
                    slice
                };
                for chunk in send.chunks(8192) {
                    if stream.write_all(chunk).is_err() {
                        return;
                    }
                    if !delay.is_zero() {
                        std::thread::sleep(delay);
                    }
                }
            });
        }
    });
    Server { url, hits }
}

fn body(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i * 31 % 251) as u8).collect()
}

fn md5_of(b: &[u8]) -> String {
    hex::encode(Md5::digest(b))
}

fn spec(id: &str, url: &str, data: &[u8]) -> DownloadSpec {
    DownloadSpec {
        id: id.into(),
        url: url.into(),
        file_name: "Cool: Mod?.zip".into(),
        title: "Cool".into(),
        expected_md5: Some(md5_of(data)),
        expected_size: Some(data.len() as u64),
        meta: serde_json::Value::Null,
    }
}

#[test]
fn downloads_verifies_and_resumes_after_cut() {
    let dir = tempfile::tempdir().unwrap();
    let data = body(300_000);
    let server = serve(data.clone(), true, Duration::ZERO);
    let events = Arc::new(Mutex::new(Vec::<Status>::new()));
    let ev = events.clone();
    let m = DownloadManager::new(dir.path(), 2, move |i| ev.lock().push(i.status));
    m.enqueue(spec("d1", &server.url, &data));
    assert!(m.wait_idle(Duration::from_secs(20)));
    let item = m.item("d1").unwrap();
    assert_eq!(item.status, Status::Completed, "{:?}", item.error);
    let path = item.path.unwrap();
    assert_eq!(path.file_name().unwrap(), "Cool_ Mod_.zip");
    assert_eq!(std::fs::read(&path).unwrap(), data);
    assert!(
        server.hits.load(Ordering::SeqCst) >= 2,
        "resumed with a second request"
    );
    let seen = events.lock().clone();
    assert!(seen.contains(&Status::Verifying));
    assert_eq!(seen.last(), Some(&Status::Completed));
}

#[test]
fn checksum_mismatch_fails_and_cleans_up() {
    let dir = tempfile::tempdir().unwrap();
    let data = body(50_000);
    let server = serve(data.clone(), false, Duration::ZERO);
    let m = DownloadManager::new(dir.path(), 1, |_| {});
    let mut s = spec("bad", &server.url, &data);
    s.expected_md5 = Some("00000000000000000000000000000000".into());
    m.enqueue(s);
    assert!(m.wait_idle(Duration::from_secs(20)));
    let item = m.item("bad").unwrap();
    assert_eq!(item.status, Status::Failed);
    assert_eq!(item.error.unwrap().code, ErrorCode::HashMismatch);
    assert!(!dir.path().join("bad.part").exists());
}

#[test]
fn pause_resume_and_cancel() {
    let dir = tempfile::tempdir().unwrap();
    let data = body(400_000);
    let server = serve(data.clone(), false, Duration::from_millis(15));
    let m = DownloadManager::new(dir.path(), 1, |_| {});
    m.enqueue(spec("p", &server.url, &data));
    // Queue a second one that must wait for the first (max 1).
    m.enqueue(spec("q", &server.url, &data));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(m.item("q").unwrap().status, Status::Queued);
    m.pause("p");
    let deadline = Instant::now() + Duration::from_secs(10);
    while m.item("p").unwrap().status != Status::Paused && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(m.item("p").unwrap().status, Status::Paused);
    let partial = std::fs::metadata(dir.path().join("p.part")).unwrap().len();
    assert!(partial > 0 && partial < data.len() as u64);
    m.cancel("q");
    m.resume("p");
    assert!(m.wait_idle(Duration::from_secs(30)));
    assert_eq!(m.item("p").unwrap().status, Status::Completed);
    assert_eq!(m.item("q").unwrap().status, Status::Cancelled);
    assert!(m.remove("q"));
}

#[test]
fn restore_requeues_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let data = body(10_000);
    let server = serve(data.clone(), false, Duration::ZERO);
    let m = DownloadManager::new(dir.path(), 2, |_| {});
    let item = DownloadItem {
        spec: spec("r", &server.url, &data),
        status: Status::Downloading,
        received: 0,
        total: None,
        bytes_per_sec: 0,
        error: None,
        path: None,
        created_at: 0,
    };
    m.restore(vec![item]);
    assert!(m.wait_idle(Duration::from_secs(20)));
    assert_eq!(m.item("r").unwrap().status, Status::Completed);
}

#[test]
fn names() {
    assert_eq!(safe_file_name("a/b\\c:d.zip"), "a_b_c_d.zip");
    assert_eq!(safe_file_name(" .. "), "download");
    assert_eq!(safe_file_name("x. "), "x");
}

#[test]
fn remove_notifies_listener() {
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(AtomicUsize::new(0));
    let s = seen.clone();
    let m = DownloadManager::new(dir.path(), 1, move |_| {
        s.fetch_add(1, Ordering::SeqCst);
    });
    let item = DownloadItem {
        spec: spec("c", "http://127.0.0.1:9/never", b"x"),
        status: Status::Cancelled,
        received: 0,
        total: None,
        bytes_per_sec: 0,
        error: None,
        path: None,
        created_at: 0,
    };
    m.restore(vec![item]);
    let before = seen.load(Ordering::SeqCst);
    assert!(m.remove("c"));
    assert!(m.item("c").is_none());
    assert_eq!(seen.load(Ordering::SeqCst), before + 1);
    assert!(!m.remove("c"));
}

/// Answers the first `fail_first` requests with HTTP `code`, then serves
/// `body`. With `stall`, a full response sends half the body and then hangs.
fn serve_with(body: Vec<u8>, code: u16, fail_first: usize, stall: bool) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/file.zip", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let body = Arc::new(body);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let n = h.fetch_add(1, Ordering::SeqCst);
            let body = body.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 && line != "\r\n" {
                    line.clear();
                }
                if n < fail_first {
                    let _ = stream.write_all(
                        format!(
                            "HTTP/1.1 {code} Nope\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                        .as_bytes(),
                    );
                    return;
                }
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                );
                if stall {
                    let _ = stream.write_all(&body[..body.len() / 2]);
                    std::thread::sleep(Duration::from_secs(120));
                } else {
                    let _ = stream.write_all(&body);
                }
            });
        }
    });
    Server { url, hits }
}

#[test]
fn cancel_and_pause_take_effect_while_the_transfer_hangs() {
    let dir = tempfile::tempdir().unwrap();
    let data = body(200_000);
    let server = serve_with(data.clone(), 200, 0, true);
    let m = DownloadManager::new(dir.path(), 1, |_| {});
    m.enqueue(spec("s", &server.url, &data));
    m.enqueue(spec("t", &server.url, &data));
    let deadline = Instant::now() + Duration::from_secs(10);
    let part = dir.path().join("s.part");
    while file_len(&part) < 100_000 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(file_len(&part), 100_000);
    // The worker is stuck in a read; pausing frees the queue at once.
    m.pause("s");
    assert_eq!(m.item("s").unwrap().status, Status::Paused);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(m.item("t").unwrap().status, Status::Downloading);
    m.cancel("t");
    assert_eq!(m.item("t").unwrap().status, Status::Cancelled);
    m.cancel("s");
    assert_eq!(m.item("s").unwrap().status, Status::Cancelled);
    assert!(m.wait_idle(Duration::from_secs(2)));
    assert!(!dir.path().join("s.part").exists());
}

#[test]
fn server_errors_are_retried() {
    let dir = tempfile::tempdir().unwrap();
    let data = body(20_000);
    let server = serve_with(data.clone(), 503, 1, false);
    let m = DownloadManager::new(dir.path(), 1, |_| {});
    m.enqueue(spec("e", &server.url, &data));
    assert!(m.wait_idle(Duration::from_secs(20)));
    let item = m.item("e").unwrap();
    assert_eq!(item.status, Status::Completed, "{:?}", item.error);
    assert_eq!(server.hits.load(Ordering::SeqCst), 2);
}

#[test]
fn missing_files_fail_without_retrying() {
    let dir = tempfile::tempdir().unwrap();
    let data = body(1_000);
    let server = serve_with(data.clone(), 404, usize::MAX, false);
    let m = DownloadManager::new(dir.path(), 1, |_| {});
    m.enqueue(spec("g", &server.url, &data));
    assert!(m.wait_idle(Duration::from_secs(5)));
    assert_eq!(m.item("g").unwrap().status, Status::Failed);
    assert_eq!(server.hits.load(Ordering::SeqCst), 1);
}

#[test]
fn a_stale_partial_file_is_downloaded_again() {
    let dir = tempfile::tempdir().unwrap();
    let data = body(30_000);
    let server = serve(data.clone(), false, Duration::ZERO);
    std::fs::write(dir.path().join("x.part"), vec![7u8; 30_000]).unwrap();
    let m = DownloadManager::new(dir.path(), 1, |_| {});
    m.enqueue(spec("x", &server.url, &data));
    assert!(m.wait_idle(Duration::from_secs(20)));
    let item = m.item("x").unwrap();
    assert_eq!(item.status, Status::Completed, "{:?}", item.error);
    assert_eq!(std::fs::read(item.path.unwrap()).unwrap(), data);
}

#[test]
fn a_failed_install_retries_without_downloading() {
    let dir = tempfile::tempdir().unwrap();
    let data = body(5_000);
    let server = serve(data.clone(), false, Duration::ZERO);
    let m = DownloadManager::new(dir.path(), 1, |_| {});
    m.enqueue(spec("i", &server.url, &data));
    assert!(m.wait_idle(Duration::from_secs(20)));
    m.fail("i", AppError::new(ErrorCode::ExtractFailed, "bad archive"));
    assert_eq!(m.item("i").unwrap().status, Status::Failed);
    m.resume("i");
    assert_eq!(m.item("i").unwrap().status, Status::Completed);
    assert_eq!(server.hits.load(Ordering::SeqCst), 1);
}
