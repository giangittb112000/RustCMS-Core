//! `rustcms serve --watch` with a stand-in `cargo` (a shell script): a failed
//! build keeps the server; a good one replaces it; Ctrl-C stops the server
//! and what it started (its process group). Unix only, as the command.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A `cargo` answering `metadata`, `build` (fails while `BROKEN` exists) and
/// `run` (a server that records its pid, starts a grandchild, and stops on
/// SIGTERM).
const FAKE_CARGO: &str = r#"#!/bin/sh
case "$1" in
  metadata)
    printf '{"workspace_root":"%s","packages":[{"name":"app","manifest_path":"%s/Cargo.toml","default_run":null,"targets":[{"name":"app","kind":["bin"]}]}]}' "$PWD" "$PWD"
    ;;
  build)
    echo build >> builds.log
    [ -e BROKEN ] && exit 1
    exit 0
    ;;
  run)
    sleep 1000 &
    echo $! > grandchild.pid
    echo $$ > server.pid
    trap 'echo stopped >> server.log; kill $(cat grandchild.pid) 2>/dev/null; exit 0' TERM
    while true; do sleep 0.1; done
    ;;
esac
"#;

fn app() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"app\"\n\n[package.metadata.rustcms]\n").unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    let cargo = root.join("fake-cargo");
    std::fs::write(&cargo, FAKE_CARGO).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

fn watcher(root: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_rustcms"))
        .args(["serve", "--watch"])
        .current_dir(root)
        .env("CARGO", root.join("fake-cargo"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .unwrap()
}

/// Waits (at most 15 s) until `check` holds.
fn until(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn pid(path: PathBuf) -> Option<i32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn alive(pid: i32) -> bool {
    Command::new("kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

fn builds(root: &Path) -> usize {
    std::fs::read_to_string(root.join("builds.log")).map(|s| s.lines().count()).unwrap_or(0)
}

/// Changes `src/main.rs` (another size, so the poll sees it whatever the clock).
fn edit(root: &Path, text: &str) {
    std::fs::write(root.join("src/main.rs"), text).unwrap();
}

#[test]
fn a_failed_build_keeps_the_server_a_good_one_replaces_it_and_ctrl_c_stops_everything() {
    let dir = app();
    let root = dir.path();
    let mut watch = watcher(root);
    until("the first server", || pid(root.join("server.pid")).is_some_and(alive));
    let first = pid(root.join("server.pid")).unwrap();
    let grandchild = pid(root.join("grandchild.pid")).unwrap();

    // A change that does not build: the server keeps running.
    std::fs::write(root.join("BROKEN"), "").unwrap();
    let before = builds(root);
    edit(root, "fn main() { broken }\n");
    until("the failed build", || builds(root) > before);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(pid(root.join("server.pid")), Some(first));
    assert!(alive(first), "the server keeps running");

    // Fixed: built again and replaced (the old server and its child stopped).
    std::fs::remove_file(root.join("BROKEN")).unwrap();
    edit(root, "fn main() { println!(\"fixed\"); }\n");
    until("a new server", || pid(root.join("server.pid")).is_some_and(|p| p != first && alive(p)));
    until("the old server to stop", || !alive(first) && !alive(grandchild));
    let second = pid(root.join("server.pid")).unwrap();
    let second_child = pid(root.join("grandchild.pid")).unwrap();

    // A template registered with `view!` reloads itself: no build.
    let before = builds(root);
    std::fs::create_dir_all(root.join("resources/views/admin")).unwrap();
    std::fs::write(root.join("resources/views/admin/page.html"), "<p>x</p>").unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(builds(root), before);

    // Ctrl-C (SIGINT to the watcher): the server and its child stop too.
    Command::new("kill").args(["-INT", &watch.id().to_string()]).status().unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = watch.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the watcher did not stop");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(status.success(), "{status}");
    assert!(!alive(second) && !alive(second_child), "nothing left behind");
}

#[test]
fn ctrl_c_during_a_build_stops_the_build_and_the_server() {
    let dir = app();
    let root = dir.path();
    // A build that takes long, started by a change while the server runs.
    let cargo = std::fs::read_to_string(root.join("fake-cargo")).unwrap();
    let slow = cargo.replace(
        "    echo build >> builds.log\n",
        "    echo build >> builds.log\n    if [ -e SLOW ]; then sleep 1000 & echo $! > slow.pid; touch slow.ready; wait; fi\n",
    );
    std::fs::write(root.join("fake-cargo"), slow).unwrap();
    let mut watch = watcher(root);
    until("the first server", || pid(root.join("server.pid")).is_some_and(alive));
    let server = pid(root.join("server.pid")).unwrap();
    std::fs::write(root.join("SLOW"), "").unwrap();
    let before = builds(root);
    edit(root, "fn main() { slow }\n");
    until("the slow build", || builds(root) > before && root.join("slow.ready").exists());
    Command::new("kill").args(["-INT", &watch.id().to_string()]).status().unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while watch.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "the watcher did not stop");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!alive(server), "the server stopped with the build");
    // The slow build's `sleep` was in the build's group: stopped as well.
    let slow = pid(root.join("slow.pid")).expect("the slow build started");
    assert!(!alive(slow), "the build's process group stopped");
}
