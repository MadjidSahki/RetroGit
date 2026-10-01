//! Single instance: `retrogit` from a terminal hands the folder to the window already open.
//!
//! The running instance listens on 127.0.0.1 (random port) and writes the port and a random
//! token to a private file in the data directory; requests without the token are ignored.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const FILE_NAME: &str = "instance.json";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Info {
    pub port: u16,
    pub token: String,
}

#[derive(Debug)]
pub enum SendError {
    /// No RetroGit is listening (not running, or a stale file).
    NoInstance,
    /// An instance answered but refused the request.
    Refused,
}

/// 128 random bits as hex (std's per-process random hash keys, mixed with time and pid).
fn new_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let part = |salt: u64| {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(salt);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        h.write_u32(std::process::id());
        h.finish()
    };
    format!("{:016x}{:016x}", part(1), part(2))
}

pub fn read_info(dir: &Path) -> Option<Info> {
    let text = std::fs::read_to_string(dir.join(FILE_NAME)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Write the instance file, readable by the current user only.
pub fn write_info(dir: &Path, info: &Info) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(FILE_NAME);
    let text = serde_json::to_string(info).map_err(std::io::Error::other)?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(&path)?.write_all(text.as_bytes())
}

/// Listening side, owned by the GUI. Removes its instance file when dropped.
pub struct Server {
    dir: PathBuf,
    token: String,
}

impl Server {
    /// Start listening; `on_open` runs on a background thread for each accepted folder.
    pub fn start(
        dir: &Path,
        on_open: impl Fn(PathBuf) + Send + 'static,
    ) -> std::io::Result<Server> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let token = new_token();
        write_info(
            dir,
            &Info {
                port,
                token: token.clone(),
            },
        )?;
        let expected = token.clone();
        std::thread::Builder::new()
            .name("retrogit-instance".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut reader = BufReader::new(&stream);
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        continue;
                    }
                    let mut parts = line.trim_end_matches(['\r', '\n']).splitn(2, '\t');
                    let (Some(tok), Some(path)) = (parts.next(), parts.next()) else {
                        continue;
                    };
                    if tok != expected || path.is_empty() {
                        continue; // no answer: the client reports a refusal
                    }
                    on_open(PathBuf::from(path));
                    let _ = (&stream).write_all(b"ok\n");
                }
            })?;
        Ok(Server {
            dir: dir.to_path_buf(),
            token,
        })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Only remove the file if it is still ours (another instance may have replaced it).
        if read_info(&self.dir).is_some_and(|i| i.token == self.token) {
            let _ = std::fs::remove_file(self.dir.join(FILE_NAME));
        }
    }
}

/// Ask the running instance (described in `dir`) to open `path`.
pub fn send(dir: &Path, path: &Path) -> Result<(), SendError> {
    let info = read_info(dir).ok_or(SendError::NoInstance)?;
    send_with(&info, path)
}

pub fn send_with(info: &Info, path: &Path) -> Result<(), SendError> {
    let addr = SocketAddr::from(([127, 0, 0, 1], info.port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(1))
        .map_err(|_| SendError::NoInstance)?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    stream
        .write_all(format!("{}\t{}\n", info.token, path.display()).as_bytes())
        .map_err(|_| SendError::NoInstance)?;
    let mut answer = String::new();
    let _ = BufReader::new(&stream).read_line(&mut answer);
    if answer.trim() == "ok" {
        Ok(())
    } else {
        Err(SendError::Refused)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    #[test]
    fn a_path_sent_to_the_running_instance_is_received() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let (tx, rx) = channel();
        let _server = Server::start(dir.path(), move |p| {
            let _ = tx.send(p);
        })
        .unwrap_or_else(|e| panic!("{e}"));
        send(dir.path(), &PathBuf::from("/w/repo")).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).ok(),
            Some(PathBuf::from("/w/repo"))
        );
    }

    #[test]
    fn a_wrong_token_is_rejected() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let (tx, rx) = channel();
        let _server = Server::start(dir.path(), move |p| {
            let _ = tx.send(p);
        })
        .unwrap_or_else(|e| panic!("{e}"));
        let mut info = read_info(dir.path()).unwrap_or_else(|| panic!("no instance file"));
        info.token = "forged".into();
        assert!(send_with(&info, &PathBuf::from("/evil")).is_err());
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    }

    #[test]
    fn without_a_running_instance_sending_fails_fast() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        assert!(matches!(
            send(dir.path(), &PathBuf::from("/r")),
            Err(SendError::NoInstance)
        ));
        // Stale file from a crashed instance: nothing listens on that port.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("{e}"));
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        drop(listener);
        write_info(
            dir.path(),
            &Info {
                port,
                token: "t".into(),
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let started = std::time::Instant::now();
        assert!(matches!(
            send(dir.path(), &PathBuf::from("/r")),
            Err(SendError::NoInstance)
        ));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn the_instance_file_is_private_and_removed_on_shutdown() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let server = Server::start(dir.path(), |_| {}).unwrap_or_else(|e| panic!("{e}"));
        let file = dir.path().join(FILE_NAME);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file)
                .map(|m| m.permissions().mode() & 0o777)
                .unwrap_or(0);
            assert_eq!(mode, 0o600);
        }
        drop(server);
        assert!(!file.exists());
    }
}
