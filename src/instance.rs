//! One GhostLane at a time: a .ghost file opened while GhostLane runs (double-clicked in Explorer, say) is handed to
//! the running window instead of starting a second one.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;

/// `<config_dir>/GhostLane/instance`: the running window's port and a secret, one per line.
fn instance_file() -> Option<PathBuf> {
    crate::config::config_dir().map(|d| d.join("instance"))
}

/// The `.ghost` file among the command-line arguments, if any.
pub fn ghost_arg(args: &[String]) -> Option<PathBuf> {
    args.iter()
        .skip(1)
        .find(|a| !a.starts_with('-') && a.to_lowercase().ends_with(".ghost"))
        .map(PathBuf::from)
}

/// Hands `ghost` to a running GhostLane. False when none answers, so this one should start.
pub fn forward(ghost: &Path) -> bool {
    let Some(text) = instance_file().and_then(|f| std::fs::read_to_string(f).ok()) else {
        return false;
    };
    send(&text, ghost).is_some()
}

/// Sends `ghost` to the instance described by `instance` (port, then secret) and waits for its "ok".
fn send(instance: &str, ghost: &Path) -> Option<()> {
    let mut lines = instance.lines();
    let port: u16 = lines.next()?.trim().parse().ok()?;
    let secret = lines.next()?.trim();
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(500)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let ghost = std::path::absolute(ghost).ok()?;
    write!(stream, "{secret}\n{}\n", ghost.display()).ok()?;
    // Done sending: the other side reads to the end, so nothing is left unread when it closes.
    stream.shutdown(Shutdown::Write).ok()?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply).ok()?;
    (reply.trim() == "ok").then_some(())
}

/// Listens for .ghost files other launches hand over and passes them to `tx`, calling `wake` after each.
/// The secret keeps out anything else on this computer that finds the port, such as a web page.
pub fn listen(tx: Sender<PathBuf>, wake: impl Fn() + Send + 'static) -> std::io::Result<()> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let secret = secret();
    let file = instance_file().ok_or_else(|| std::io::Error::other("no config directory"))?;
    crate::ticket::write_atomic(&file, &format!("{}\n{secret}\n", listener.local_addr()?.port()))?;
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if let Some(ghost) = receive(stream, &secret) {
                let _ = tx.send(ghost);
                wake();
            }
        }
    });
    Ok(())
}

/// Reads one handed-over path from `stream`, answering "ok" when the secret matches.
fn receive(mut stream: TcpStream, secret: &str) -> Option<PathBuf> {
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut request = String::new();
    (&mut stream).take(8192).read_to_string(&mut request).ok()?;
    let mut lines = request.lines();
    let ghost = (lines.next()?.trim() == secret).then(|| PathBuf::from(lines.next().unwrap_or("").trim()));
    let ghost = ghost.filter(|g| g.extension().is_some_and(|e| e.eq_ignore_ascii_case("ghost")));
    let _ = stream.write_all(if ghost.is_some() { b"ok" } else { b"no" });
    let _ = stream.shutdown(Shutdown::Write);
    ghost
}

/// 128 random bits as hex, from the standard library's randomly keyed hasher.
fn secret() -> String {
    use std::hash::{BuildHasher, Hasher};
    (0..2)
        .map(|_| {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u128(std::time::SystemTime::now().elapsed().map_or(0, |d| d.as_nanos()));
            format!("{:016x}", h.finish())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_ghost_argument() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(ghost_arg(&args(&["gl"])), None);
        assert_eq!(
            ghost_arg(&args(&["gl", "--x", r"C:\p\My.GHOST"])),
            Some(PathBuf::from(r"C:\p\My.GHOST"))
        );
        assert_eq!(ghost_arg(&args(&["gl", "notes.md"])), None);
    }

    #[test]
    fn hands_paths_over_only_with_the_secret() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                receive(stream, "s3cret");
            }
        });
        assert!(send(&format!("{port}\ns3cret\n"), Path::new("x/p.ghost")).is_some());
        assert!(send(&format!("{port}\nwrong\n"), Path::new("x/p.ghost")).is_none());
        assert!(send(&format!("{port}\ns3cret\n"), Path::new("x/p.txt")).is_none());
        assert!(send("garbage", Path::new("x/p.ghost")).is_none());
        assert_ne!(secret(), secret());
        assert_eq!(secret().len(), 32);
    }
}
