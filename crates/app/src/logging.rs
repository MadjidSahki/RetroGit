//! Tiny file logger that never writes a token in clear.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

pub const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

const TOKEN_PREFIXES: &[&str] = &["github_pat_", "ghp_", "gho_", "ghu_", "ghs_", "ghr_"];

/// Mask known secrets and anything that looks like a GitHub token.
pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for s in secrets.iter().filter(|s| !s.is_empty()) {
        out = out.replace(s.as_str(), "***");
    }
    for prefix in TOKEN_PREFIXES {
        let mut result = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(i) = rest.find(prefix) {
            result.push_str(&rest[..i]);
            let after = &rest[i + prefix.len()..];
            let len = after
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            if len == 0 {
                result.push_str(prefix);
            } else {
                result.push_str("***");
            }
            rest = &after[len..];
        }
        result.push_str(rest);
        out = result;
    }
    out
}

/// Open the log for appending, truncating it first if it grew past `max_bytes`.
pub fn open_log_file(path: &Path, max_bytes: u64) -> std::io::Result<File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let too_big = std::fs::metadata(path)
        .map(|m| m.len() > max_bytes)
        .unwrap_or(false);
    OpenOptions::new()
        .create(true)
        .append(!too_big)
        .write(true)
        .truncate(too_big)
        .open(path)
}

struct FileLogger {
    file: Mutex<File>,
}

static SECRETS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static LOGGER: OnceLock<FileLogger> = OnceLock::new();

/// Register a secret that must never reach the log (e.g. the current token).
pub fn add_secret(secret: &str) {
    if let Ok(mut s) = SECRETS.lock()
        && !s.iter().any(|x| x == secret)
    {
        s.push(secret.to_string());
    }
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let secrets = SECRETS.lock().map(|s| s.clone()).unwrap_or_default();
        let line = redact(&format!("{}", record.args()), &secrets);
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if let Ok(mut f) = self.file.lock() {
            let _ = writeln!(
                f,
                "{} {:<5} {}",
                crate::format::format_epoch(secs),
                record.level(),
                line
            );
        }
    }

    fn flush(&self) {
        if let Ok(mut f) = self.file.lock() {
            let _ = f.flush();
        }
    }
}

/// Log every panic (UI or worker thread) with its location, token-shaped text masked.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let secrets = SECRETS.lock().map(|s| s.clone()).unwrap_or_default();
        log::error!("panic: {}", redact(&panic_text(info), &secrets));
        previous(info);
    }));
}

/// `message at file:line` for a panic.
pub fn panic_text(info: &std::panic::PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    let message = payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(non-text panic)".into());
    match info.location() {
        Some(l) => format!("{message} at {}:{}", l.file(), l.line()),
        None => message,
    }
}

/// Install the file logger. Failing to open the log is not fatal.
pub fn init(path: &Path) {
    let Ok(file) = open_log_file(path, MAX_LOG_BYTES) else {
        return;
    };
    let logger = LOGGER.get_or_init(|| FileLogger {
        file: Mutex::new(file),
    });
    if log::set_logger(logger).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn redacts_registered_secrets_and_token_shapes() {
        let secrets = vec!["s3cr3t".to_string()];
        assert_eq!(redact("pw=s3cr3t!", &secrets), "pw=***!");
        assert_eq!(redact("token gho_AbC123 end", &[]), "token *** end");
        assert_eq!(redact("github_pat_11AA_bb", &[]), "***");
        assert_eq!(redact("ghp_", &[]), "ghp_");
        assert_eq!(redact("a ghs_x b ghu_y", &[]), "a *** b ***");
        assert_eq!(redact("nothing here", &[]), "nothing here");
    }

    #[test]
    fn log_file_is_truncated_when_too_big() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("logs").join("retrogit.log");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, vec![b'x'; 100]).unwrap();
        drop(open_log_file(&p, 1000).unwrap());
        assert_eq!(std::fs::metadata(&p).unwrap().len(), 100);
        drop(open_log_file(&p, 10).unwrap());
        assert_eq!(std::fs::metadata(&p).unwrap().len(), 0);
    }
}
