//! File paths without the people in them (docs/design.md §4.3): paths from
//! build machines (`/Users/<name>/…`, `/home/<name>/…`, `C:\Users\<name>\…`,
//! CI workspaces) become a path relative to the project, or at least one
//! without the home directory, so no user name is stored. It applies to the
//! paths the SDKs send and to the ones symbolication takes from a dSYM.
//!
//! Only the start of a path changes: its file name stays, so grouping (which
//! uses base names) and the in-app rules see the same file.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

/// CI workspaces: everything up to the repository's own folder goes.
static CI_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^(?:
            /(?:home|Users)/runner/work/[^/]+/[^/]+/      # GitHub Actions (Linux, macOS)
          | [A-Za-z]:\\a\\[^\\]+\\[^\\]+\\                 # GitHub Actions (Windows)
          | /Volumes/workspace/repository/                # Xcode Cloud
          | /Users/vagrant/git/                           # Bitrise
          | /(?:home/circleci|Users/distiller)/project/   # CircleCI
          | /builds/[^/]+/[^/]+/                          # GitLab
          | .*?/jenkins/workspace/[^/]+/                  # Jenkins
          | /(?:var/lib|home)/jenkins/(?:workspace/)?[^/]+/
          | /opt/build/repo/                              # Netlify
          | /vercel/path\d+/                              # Vercel
        )",
    )
    .expect("valid regex")
});

/// A home directory: `/Users/<name>/`, `/home/<name>/`, `C:\Users\<name>\`,
/// `/c/Users/<name>/`, `/mnt/c/Users/<name>/`.
static HOME_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)^(?:
            /(?:mnt/[a-z]/|[a-z]/)?(?:Users|users|home)/[^/]+/
          | [A-Za-z]:[\\/](?:Users|Documents\ and\ Settings)[\\/][^\\/]+[\\/]
        )",
    )
    .expect("valid regex")
});

/// Folders people keep their code in, dropped from the start of what is
/// left of a home directory (`Developer/Shop/…` becomes `Shop/…`).
const CODE_FOLDERS: &[&str] = &[
    "Developer",
    "Documents",
    "Desktop",
    "Projects",
    "projects",
    "Code",
    "code",
    "src",
    "Source",
    "source",
    "repos",
    "Repos",
    "git",
    "Git",
    "GitHub",
    "github",
    "workspace",
    "Workspace",
    "work",
    "Work",
    "dev",
    "Dev",
];

/// `path` without a build machine's home directory or CI workspace, or
/// `None` when it has neither.
pub fn reduce(path: &str) -> Option<String> {
    if let Some(m) = CI_PREFIX.find(path) {
        return Some(non_empty(&path[m.end()..], path));
    }
    let m = HOME_PREFIX.find(path)?;
    let mut rest = &path[m.end()..];
    for _ in 0..3 {
        let Some((first, tail)) = rest.split_once(['/', '\\']) else {
            break;
        };
        if CODE_FOLDERS.contains(&first) && !tail.is_empty() {
            rest = tail;
        } else {
            break;
        }
    }
    Some(non_empty(rest, path))
}

/// `rest`, or the file name of `path` when nothing is left.
fn non_empty(rest: &str, path: &str) -> String {
    if rest.trim_matches(['/', '\\']).is_empty() {
        crate::event::grouping::basename(path).to_string()
    } else {
        rest.to_string()
    }
}

fn reduce_field(map: &mut Map<String, Value>, key: &str) -> usize {
    let Some(Value::String(s)) = map.get(key) else {
        return 0;
    };
    match reduce(s) {
        Some(short) => {
            map.insert(key.into(), Value::String(short));
            1
        }
        None => 0,
    }
}

fn frames_of(holder: &mut Value) -> impl Iterator<Item = &mut Map<String, Value>> {
    holder
        .get_mut("stacktrace")
        .and_then(|s| s.get_mut("frames"))
        .and_then(Value::as_array_mut)
        .into_iter()
        .flat_map(|frames| frames.iter_mut().filter_map(Value::as_object_mut))
}

/// Reduces the paths of every frame (`abs_path`, `filename`, `package`) and
/// of the loaded images (`code_file`, `debug_file`), in place. Returns how
/// many it changed.
pub fn reduce_event(event: &mut Map<String, Value>) -> usize {
    let mut n = 0;
    let mut frame = |f: &mut Map<String, Value>| {
        for key in ["abs_path", "filename", "package"] {
            n += reduce_field(f, key);
        }
    };
    for key in ["exception", "threads"] {
        if let Some(values) = event
            .get_mut(key)
            .and_then(|v| v.get_mut("values"))
            .and_then(Value::as_array_mut)
        {
            for holder in values.iter_mut() {
                frames_of(holder).for_each(&mut frame);
            }
        }
    }
    if let Some(frames) = event
        .get_mut("stacktrace")
        .and_then(|s| s.get_mut("frames"))
        .and_then(Value::as_array_mut)
    {
        frames
            .iter_mut()
            .filter_map(Value::as_object_mut)
            .for_each(&mut frame);
    }
    if let Some(images) = event
        .get_mut("debug_meta")
        .and_then(|d| d.get_mut("images"))
        .and_then(Value::as_array_mut)
    {
        for image in images.iter_mut().filter_map(Value::as_object_mut) {
            n += reduce_field(image, "code_file");
            n += reduce_field(image, "debug_file");
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn build_machine_paths_lose_the_user() {
        for (raw, want) in [
            (
                "/Users/runner/work/kestrel-spike/kestrel-spike/ios/KestrelSpike/CrashSite.swift",
                "ios/KestrelSpike/CrashSite.swift",
            ),
            ("/home/runner/work/shop/shop/src/main.rs", "src/main.rs"),
            ("D:\\a\\shop\\shop\\src\\main.rs", "src\\main.rs"),
            (
                "/Volumes/workspace/repository/App/Sources/App.swift",
                "App/Sources/App.swift",
            ),
            (
                "/Users/vagrant/git/App/AppDelegate.swift",
                "App/AppDelegate.swift",
            ),
            ("/builds/acme/shop/app/views.py", "app/views.py"),
            (
                "/var/lib/jenkins/workspace/shop-main/src/index.js",
                "src/index.js",
            ),
            (
                "/Users/adalovelace/Developer/Shop/Shop/Checkout.swift",
                "Shop/Shop/Checkout.swift",
            ),
            ("/home/ada/projects/shop/app/views.py", "shop/app/views.py"),
            (
                "C:\\Users\\Ada Lovelace\\source\\repos\\Shop\\Program.cs",
                "Shop\\Program.cs",
            ),
            ("/mnt/c/Users/ada/code/shop/x.ts", "shop/x.ts"),
            (
                "/Users/ada/Library/Developer/CoreSimulator/Devices/1/data/App.app/App",
                "Library/Developer/CoreSimulator/Devices/1/data/App.app/App",
            ),
            ("/Users/ada/main.swift", "main.swift"),
            ("/Users/ada/Developer/main.swift", "main.swift"),
        ] {
            assert_eq!(reduce(raw).as_deref(), Some(want), "{raw}");
        }
        for kept in [
            "/app/views.py",
            "/usr/local/lib/python3.13/site-packages/flask/app.py",
            "/private/var/containers/Bundle/Application/X/App.app/App",
            "app:///main.js",
            "views.py",
            "/usr/local/cargo/registry/src/index.crates.io-1949cf8c6b5b557f/sentry-core-0.49.3/src/hub.rs",
        ] {
            assert_eq!(reduce(kept), None, "{kept}");
        }
    }

    #[test]
    fn events_lose_home_directories_everywhere() {
        let mut e = json!({
            "exception": {"values": [{"stacktrace": {"frames": [
                {"abs_path": "/Users/ada/Developer/Shop/Checkout.swift", "filename": "Checkout.swift",
                 "package": "/Users/ada/Library/Developer/CoreSimulator/App.app/App"}]}}]},
            "threads": {"values": [{"stacktrace": {"frames": [{"filename": "/home/ada/shop/app.js"}]}}]},
            "debug_meta": {"images": [{"code_file": "/Users/ada/Library/Developer/X.app/X", "debug_file": "/Users/ada/X.dSYM"}]},
        });
        let n = reduce_event(e.as_object_mut().unwrap());
        assert_eq!(n, 5);
        assert!(!e.to_string().contains("ada"), "{e}");
        assert_eq!(
            e["exception"]["values"][0]["stacktrace"]["frames"][0]["abs_path"],
            "Shop/Checkout.swift"
        );
        assert_eq!(
            e["exception"]["values"][0]["stacktrace"]["frames"][0]["filename"],
            "Checkout.swift"
        );
    }
}
