//! Startup check for a newer GitHub release. Uses the system `curl` (bundled with Windows 10+,
//! macOS and most Linux distros) so the app needs no HTTP stack; no curl means no check.

use std::process::Command;

pub const RELEASES_URL: &str = "https://github.com/EdTheDuck/GhostLane/releases/latest";

/// Newest released version if it's newer than this build, e.g. `"0.2.0"`. Blocks on the network.
pub fn newer_release() -> Option<String> {
    let mut cmd = Command::new("curl");
    // `/releases/latest` redirects to `/releases/tag/v<version>`; the header is all we need.
    cmd.args(["-sI", "--max-time", "10", RELEASES_URL]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().ok()?;
    let latest = tag_from_headers(&String::from_utf8_lossy(&out.stdout))?;
    (parse(&latest)? > parse(env!("CARGO_PKG_VERSION"))?).then_some(latest)
}

fn tag_from_headers(headers: &str) -> Option<String> {
    let location = headers
        .lines()
        .find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("location")))?
        .1;
    let tag = location.trim().rsplit_once("/releases/tag/")?.1;
    Some(tag.strip_prefix('v').unwrap_or(tag).to_string())
}

/// `"1.2.3"` -> `(1, 2, 3)`. Pre-release or odd tags are ignored.
fn parse(v: &str) -> Option<(u32, u32, u32)> {
    let mut parts = v.split('.').map(|p| p.parse().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tag_from_redirect() {
        let headers =
            "HTTP/2 302\r\ndate: x\r\nLocation: https://github.com/EdTheDuck/GhostLane/releases/tag/v0.10.2\r\n\r\n";
        assert_eq!(tag_from_headers(headers).as_deref(), Some("0.10.2"));
        // No releases yet: GitHub redirects to the release list instead.
        assert_eq!(tag_from_headers("location: https://github.com/a/b/releases\r\n"), None);
        assert_eq!(tag_from_headers(""), None);
    }

    #[test]
    fn compares_versions_numerically() {
        assert!(parse("0.10.0") > parse("0.9.9"));
        assert!(parse("1.0.0") > parse("0.99.99"));
        assert_eq!(parse("1.2"), None);
        assert_eq!(parse("1.2.3-beta"), None);
        assert_eq!(parse("1.2.3.4"), None);
    }
}
