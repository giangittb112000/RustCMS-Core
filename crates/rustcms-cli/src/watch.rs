//! `rustcms serve --watch`: runs the application's `serve`, then rebuilds and
//! restarts it when one of its files changes.
//!
//! Files are polled (modification time and size), without a file-watching
//! dependency: an application's sources are small. A build that fails leaves
//! the running server as it was; the next change tries again.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::{Duration, Instant, SystemTime};

use crate::project::Project;

/// What the application is built from: directories, then files, under its root.
const WATCHED_DIRS: &[&str] = &["src", "config", "lang", "resources"];
const WATCHED_FILES: &[&str] = &["Cargo.toml", "Cargo.lock", "build.rs", ".env"];
/// Directories never looked into (build output, dependencies, VCS).
const SKIPPED: &[&str] = &["target", "node_modules", ".git"];

const POLL: Duration = Duration::from_millis(500);
/// Changes come in bursts (an editor saving several files, `cargo fmt`): wait
/// until the files are quiet this long before building.
const SETTLE: Duration = Duration::from_millis(300);
/// How long a stopping server may drain its requests before it is killed.
const GRACE: Duration = Duration::from_secs(10);

/// Modification time and size of each watched file.
type Snapshot = BTreeMap<PathBuf, (SystemTime, u64)>;

/// Whether `serve`'s arguments ask for watching: `--watch` before any `--`.
pub fn requested(args: &[String]) -> bool {
    args.iter().take_while(|a| *a != "--").any(|a| a == "--watch")
}

/// `serve`'s arguments without the `--watch` flags given before any `--`.
pub fn without_flag(args: &[String]) -> Vec<String> {
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    args[..end].iter().filter(|a| *a != "--watch").chain(&args[end..]).cloned().collect()
}

/// Runs `serve` with `args` (without `--watch`), rebuilding and restarting it
/// on changes until interrupted (Ctrl-C stops the server and this loop).
pub fn run(project: &Project, args: &[String]) -> anyhow::Result<ExitCode> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    println!("watching {} for changes (Ctrl-C to stop)", watched().join(", "));
    let mut seen = snapshot(&project.root);
    let mut server = match build(project, &cargo, &mut seen) {
        Some(binary) => start(project, &binary, args),
        None => {
            println!("build failed; waiting for a change");
            None
        }
    };
    loop {
        std::thread::sleep(POLL);
        if let Some(child) = server.as_mut()
            && let Some(status) = child.try_wait()?
        {
            println!("server stopped ({status}); waiting for a change to start it again");
            server = None;
        }
        let now = snapshot(&project.root);
        if now == seen {
            continue;
        }
        let now = settle(&project.root, now);
        for path in changed(&seen, &now).iter().take(3) {
            println!("changed: {}", path.strip_prefix(&project.root).unwrap_or(path).display());
        }
        seen = now;
        let Some(binary) = build(project, &cargo, &mut seen) else {
            if server.is_some() {
                println!("build failed; the server keeps running the previous build");
            } else {
                println!("build failed; no server is running, waiting for a change");
            }
            continue;
        };
        if let Some(child) = server.take() {
            stop(child);
        }
        server = start(project, &binary, args);
    }
}

/// What is watched, as printed: directories, then files.
fn watched() -> Vec<String> {
    WATCHED_DIRS.iter().map(|d| format!("{d}/")).chain(WATCHED_FILES.iter().map(|f| (*f).to_owned())).collect()
}

/// `cargo build` of the application binary; the path of the binary built.
/// Diagnostics go to the terminal; cargo's JSON messages name the binary.
/// The build's own write of `Cargo.lock` (after a dependency change) is
/// taken into `seen`, so it does not trigger another build.
fn build(project: &Project, cargo: &str, seen: &mut Snapshot) -> Option<PathBuf> {
    let lock = project.root.join("Cargo.lock");
    let lock_before = seen.get(&lock).copied();
    let started = Instant::now();
    let output = Command::new(cargo)
        .current_dir(&project.root)
        .args(["build", "--quiet", "--message-format=json-render-diagnostics"])
        .args(["--package", &project.package, "--bin", &project.package])
        .stderr(Stdio::inherit())
        .output();
    let output = match output {
        Ok(output) => output,
        Err(err) => {
            eprintln!("cannot run cargo: {err}");
            return None;
        }
    };
    let mut lock_after = Snapshot::new();
    record(&lock, &mut lock_after);
    if lock_after.get(&lock).copied() != lock_before {
        match lock_after.remove(&lock) {
            Some(stamp) => seen.insert(lock, stamp),
            None => seen.remove(&lock),
        };
    }
    if !output.status.success() {
        return None;
    }
    let binary = executable(&String::from_utf8_lossy(&output.stdout), &project.package);
    match &binary {
        Some(_) => println!("built in {:.1}s", started.elapsed().as_secs_f64()),
        None => eprintln!("cargo built no binary named `{}`", project.package),
    }
    binary
}

/// The executable of binary `name` in cargo's JSON messages.
fn executable(messages: &str, name: &str) -> Option<PathBuf> {
    messages
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|m| m["reason"] == "compiler-artifact" && m["target"]["name"] == name)
        .filter(|m| m["target"]["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == "bin")))
        .find_map(|m| m["executable"].as_str().map(PathBuf::from))
}

/// Starts `serve` from the binary just built, directly: its process is the
/// server, so [`stop`] reaches it on every platform (`cargo run` would put
/// cargo in between on Windows).
/// A server that cannot start is reported like a failed build: the next
/// change tries again.
fn start(project: &Project, binary: &Path, args: &[String]) -> Option<Child> {
    let child = Command::new(binary).current_dir(&project.root).arg("serve").args(args).stdin(Stdio::null()).spawn();
    child.inspect_err(|err| eprintln!("cannot start {}: {err}; waiting for a change", binary.display())).ok()
}

/// Asks the server to stop (SIGTERM on Unix, so it drains as on a deploy),
/// then kills it after [`GRACE`].
fn stop(mut child: Child) {
    #[cfg(unix)]
    {
        let asked = Command::new("kill").args(["-TERM", &child.id().to_string()]).status().is_ok_and(|s| s.success());
        if asked {
            let deadline = Instant::now() + GRACE;
            while Instant::now() < deadline {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Polls until two snapshots [`SETTLE`] apart agree; returns the last one.
fn settle(root: &Path, mut last: Snapshot) -> Snapshot {
    loop {
        std::thread::sleep(SETTLE);
        let now = snapshot(root);
        if now == last {
            return now;
        }
        last = now;
    }
}

/// The watched files under `root`.
fn snapshot(root: &Path) -> Snapshot {
    let mut out = Snapshot::new();
    for dir in WATCHED_DIRS {
        walk(&root.join(dir), &mut out);
    }
    for file in WATCHED_FILES {
        record(&root.join(file), &mut out);
    }
    out
}

fn walk(dir: &Path, out: &mut Snapshot) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !SKIPPED.contains(&name.as_ref()) {
                walk(&path, out);
            }
        } else {
            record(&path, out);
        }
    }
}

fn record(path: &Path, out: &mut Snapshot) {
    let Ok(meta) = std::fs::metadata(path) else { return };
    // Editors' swap and backup files change on every keystroke: not sources.
    let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
    if name.ends_with('~') || name.ends_with(".swp") || name.starts_with(".#") {
        return;
    }
    out.insert(path.to_path_buf(), (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len()));
}

/// Files added, removed or changed between two snapshots.
fn changed<'a>(before: &'a Snapshot, after: &'a Snapshot) -> Vec<&'a PathBuf> {
    let mut out: Vec<&PathBuf> =
        after.iter().filter(|(path, stamp)| before.get(*path) != Some(stamp)).map(|(p, _)| p).collect();
    out.extend(before.keys().filter(|path| !after.contains_key(*path)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn sees_sources_and_configuration_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "src/main.rs", "fn main() {}");
        write(root, "lang/vi.toml", "");
        write(root, ".env", "APP_KEY=x");
        write(root, "Cargo.toml", "[package]");
        write(root, "target/debug/app", "binary");
        write(root, "resources/admin/node_modules/x/index.js", "");
        write(root, "src/.main.rs.swp", "");
        write(root, "tests/feature/main.rs", "");
        let seen: Vec<String> =
            snapshot(root).keys().map(|p| p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(seen, [".env", "Cargo.toml", "lang/vi.toml", "src/main.rs"]);
    }

    #[test]
    fn watch_is_a_flag_of_serve_before_the_separator() {
        let args = |list: &[&str]| list.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
        assert!(requested(&args(&["--api-only", "--watch"])));
        assert!(!requested(&args(&["--", "--watch"])));
        assert_eq!(without_flag(&args(&["--watch", "--api-only"])), args(&["--api-only"]));
        assert_eq!(without_flag(&args(&["--watch", "--", "--watch"])), args(&["--", "--watch"]));
    }

    #[test]
    fn finds_the_binary_in_cargo_messages() {
        let messages = [
            r#"{"reason":"compiler-artifact","target":{"name":"blog","kind":["lib"]},"executable":null}"#,
            r#"{"reason":"compiler-artifact","target":{"name":"helper","kind":["bin"]},"executable":"/t/helper"}"#,
            r#"{"reason":"compiler-artifact","target":{"name":"blog","kind":["bin"]},"executable":"/t/debug/blog"}"#,
            r#"{"reason":"build-finished","success":true}"#,
        ]
        .join("\n");
        assert_eq!(executable(&messages, "blog"), Some(PathBuf::from("/t/debug/blog")));
        assert_eq!(executable(&messages, "other"), None);
    }

    #[test]
    fn the_build_s_own_lockfile_write_is_not_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "Cargo.toml", "[package]\nname = \"blog\"");
        write(root, "Cargo.lock", "version = 4");
        // A "cargo" that rewrites the lockfile and fails, as a build after a dependency change can.
        let cargo = root.join("fake-cargo");
        write(root, "fake-cargo", "#!/bin/sh\necho 'version = 4 # updated' > Cargo.lock\nexit 1\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
            let project = Project { root: root.to_path_buf(), package: "blog".into() };
            let mut seen = snapshot(root);
            assert_eq!(build(&project, cargo.to_str().unwrap(), &mut seen), None);
            assert_eq!(seen, snapshot(root), "the next poll sees no change");
        }
    }

    #[test]
    fn reports_added_changed_and_removed_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "src/a.rs", "a");
        write(root, "src/b.rs", "b");
        let before = snapshot(root);
        write(root, "src/a.rs", "a, longer");
        std::fs::remove_file(root.join("src/b.rs")).unwrap();
        write(root, "src/c.rs", "c");
        let after = snapshot(root);
        let mut names: Vec<_> = changed(&before, &after)
            .into_iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["a.rs", "b.rs", "c.rs"]);
        assert!(changed(&after, &snapshot(root)).is_empty());
    }
}
