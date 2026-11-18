//! Single instance + external requests (deep links, "Open with", CLI paths).
//!
//! The first instance listens on a loopback port; later launches forward
//! their arguments there (authenticated with a random token stored in the
//! user's data dir) and exit. Arguments are queued and drained by the app
//! tick, where they are parsed with `kombini_gamebanana::links::parse`.

use parking_lot::Mutex;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const PORT: u16 = 47_615;
const MAX_MESSAGE: u64 = 64 * 1024;

#[derive(Clone, Default)]
pub struct Inbox(pub Arc<Mutex<Vec<String>>>);

impl Inbox {
    pub fn push_all(&self, args: impl IntoIterator<Item = String>) {
        self.0.lock().extend(args);
    }
    pub fn drain(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock())
    }
}

fn token_path(data: &Path) -> PathBuf {
    data.join("instance.token")
}

pub enum Start {
    /// This is the primary instance; requests arrive in the inbox.
    Primary(Inbox),
    /// Arguments were handed to the running instance; exit now.
    Forwarded,
}

/// Becomes the primary instance or forwards `args` to it.
pub fn start(data: &Path, args: Vec<String>) -> Start {
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, PORT);
    match TcpListener::bind(addr) {
        Ok(listener) => {
            let token = random_token();
            let _ = std::fs::create_dir_all(data);
            if let Err(e) = std::fs::write(token_path(data), &token) {
                tracing::warn!(error = %e, "could not write instance token");
            }
            let inbox = Inbox::default();
            inbox.push_all(args);
            let rx = inbox.clone();
            std::thread::Builder::new()
                .name("kombini-instance".into())
                .spawn(move || serve(listener, &token, &rx))
                .ok();
            Start::Primary(inbox)
        }
        Err(_) => match forward(data, addr, &args) {
            Ok(()) => Start::Forwarded,
            Err(e) => {
                // Port taken by something else: run standalone.
                tracing::warn!(error = %e, "could not reach a running instance");
                let inbox = Inbox::default();
                inbox.push_all(args);
                Start::Primary(inbox)
            }
        },
    }
}

fn random_token() -> String {
    kombini_storage::random_hex(16)
}

fn serve(listener: TcpListener, token: &str, inbox: &Inbox) {
    for stream in listener.incoming().flatten() {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut text = String::new();
        if stream.take(MAX_MESSAGE).read_to_string(&mut text).is_err() {
            continue;
        }
        let Some((first, rest)) = text.split_once('\n') else {
            continue;
        };
        if first.trim() != token {
            tracing::warn!("ignored a forwarded request with a bad token");
            continue;
        }
        match serde_json::from_str::<Vec<String>>(rest) {
            Ok(args) => inbox.push_all(args.into_iter().take(32)),
            Err(e) => tracing::warn!(error = %e, "bad forwarded request"),
        }
    }
}

fn forward(data: &Path, addr: SocketAddrV4, args: &[String]) -> std::io::Result<()> {
    let token = std::fs::read_to_string(token_path(data))?;
    let mut stream = TcpStream::connect_timeout(&addr.into(), Duration::from_secs(2))?;
    let body = serde_json::to_string(args).map_err(std::io::Error::other)?;
    stream.write_all(format!("{}\n{body}", token.trim()).as_bytes())?;
    stream.flush()
}

/// Registers (or removes) the `kombini://` and `deadlockmm://` handlers for
/// the current user only (HKCU). Opt-in from Settings.
#[cfg(windows)]
pub fn set_url_handlers(enable: bool) -> std::io::Result<()> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let classes = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags("Software\\Classes", winreg::enums::KEY_ALL_ACCESS)?;
    let exe = std::env::current_exe()?;
    for scheme in kombini_gamebanana::links::SCHEMES {
        if enable {
            let (key, _) = classes.create_subkey(scheme)?;
            key.set_value("", &"URL:Kombini Mod Manager")?;
            key.set_value("URL Protocol", &"")?;
            let (cmd, _) = key.create_subkey("shell\\open\\command")?;
            cmd.set_value("", &format!("\"{}\" \"%1\"", exe.display()))?;
        } else {
            match classes.delete_subkey_all(scheme) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn set_url_handlers(_enable: bool) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "Link handlers are only supported on Windows",
    ))
}

#[cfg(test)]
mod tests {
    use super::Inbox;

    #[test]
    fn inbox_drains() {
        let inbox = Inbox::default();
        inbox.push_all(vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(inbox.drain(), vec!["a", "b"]);
        assert!(inbox.drain().is_empty());
    }
}
