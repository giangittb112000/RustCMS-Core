//! `rustcms serve --watch`: runs the application's `serve`, then rebuilds it
//! and replaces the server when its files change (design:
//! `docs/core/plans/serve-watch.md`; first written by @giangittb112000).
//!
//! - Files are polled (modification time and size, every 500 ms), without a
//!   file-watching dependency; changes are taken once the files are quiet.
//! - Every change is checked by `cargo build` while the old server keeps
//!   running: a build that fails leaves it as it was. Templates registered
//!   with `view!`/`mail_template!` reload themselves (debug builds, `local`),
//!   so `resources/views/` is not watched.
//! - The server runs through `cargo run`, as `rustcms serve` does (Cargo's
//!   `[env]`, runner and library paths apply); on Unix Cargo becomes the
//!   server's process.
//! - Each child (a build, the server) runs in its own process group; Ctrl-C
//!   and SIGTERM reach the watcher, which stops both groups (SIGTERM, then
//!   SIGKILL after 6 s) and reaps them before exiting.
//! - Unix only for now (Windows needs job objects to stop a process tree).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

use anyhow::{Context, bail};

use crate::project::Project;

/// How often the files are polled.
const POLL: Duration = Duration::from_millis(500);
/// Changes come in bursts (an editor saving several files, a generator):
/// they are taken once two scans this far apart agree.
const SETTLE: Duration = Duration::from_millis(300);
/// The longest a stopping child may take before it is killed (the server's
/// own drain is 5 s by default).
const GRACE: Duration = Duration::from_secs(6);

/// Directories of the application watched, then single files (relative to the
/// application's root); the workspace root adds its manifest, lockfile,
/// `.cargo/` and toolchain files.
const APP_DIRS: &[&str] = &["src", "config", "lang", "resources", ".cargo"];
const APP_FILES: &[&str] = &["Cargo.toml", "Cargo.lock", "build.rs", ".env", "rust-toolchain", "rust-toolchain.toml"];
const WORKSPACE_ENTRIES: &[&str] = &["Cargo.toml", "Cargo.lock", ".cargo", "rust-toolchain", "rust-toolchain.toml"];
/// Never looked into: build output, dependencies, VCS, runtime files, and
/// templates that reload themselves.
const SKIPPED_DIRS: &[&str] = &["target", "node_modules", ".git", "storage"];
const SKIPPED_PATHS: &[&str] = &["resources/views"];
/// An extension's sources: `npm run build` turns them into the `js`/`css`
/// the application embeds (other files there, such as a logo, are embedded
/// as they are).
const EXTENSION_SOURCES: &[&str] = &[
    "resources/admin/src",
    "resources/admin/sdk",
    "resources/admin/package.json",
    "resources/admin/package-lock.json",
    "resources/admin/tsconfig.json",
    "resources/admin/vite.config.ts",
    "resources/admin/vite.config.js",
];

/// Modification time and size of each watched file.
pub(crate) type Snapshot = BTreeMap<PathBuf, (SystemTime, u64)>;

/// Whether `serve`'s arguments ask for watching: `--watch` before any `--`.
pub fn requested(args: &[String]) -> bool {
    args.iter().take_while(|a| *a != "--").any(|a| a == "--watch")
}

/// `serve`'s arguments without the `--watch` flags given before any `--`.
pub fn without_flag(args: &[String]) -> Vec<String> {
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    args[..end].iter().filter(|a| *a != "--watch").chain(&args[end..]).cloned().collect()
}

/// Runs `serve` with `args` (without `--watch`), rebuilding and replacing it
/// on changes until interrupted.
#[cfg(not(unix))]
pub fn run(_project: &Project, _args: &[String]) -> anyhow::Result<ExitCode> {
    bail!("`serve --watch` runs on macOS and Linux for now; on Windows, run `rustcms serve` again after a change")
}

/// Runs `serve` with `args` (without `--watch`), rebuilding and replacing it
/// on changes until interrupted.
#[cfg(unix)]
pub fn run(project: &Project, args: &[String]) -> anyhow::Result<ExitCode> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(unix::watch(project, args))
}

/// The package and binary `cargo run` starts, and the workspace root.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Target {
    pub package: String,
    pub bin: String,
    pub workspace_root: PathBuf,
}

impl Target {
    /// The binary of the package whose manifest is `manifest`: its only one,
    /// or its `default-run`.
    pub(crate) fn from_metadata(metadata: &serde_json::Value, manifest: &Path) -> anyhow::Result<Self> {
        let same = |path: &str| {
            Path::new(path) == manifest || std::fs::canonicalize(path).ok() == std::fs::canonicalize(manifest).ok()
        };
        let package = metadata["packages"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|p| p["manifest_path"].as_str().is_some_and(same))
            .with_context(|| format!("cargo metadata lists no package for {}", manifest.display()))?;
        let name = package["name"].as_str().context("a package without a name")?;
        let bins: Vec<&str> = package["targets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == "bin")))
            .filter_map(|t| t["name"].as_str())
            .collect();
        let bin = match (bins.as_slice(), package["default_run"].as_str()) {
            ([only], _) => (*only).to_owned(),
            (_, Some(default)) if bins.contains(&default) => default.to_owned(),
            ([], _) => bail!("package `{name}` has no binary to serve"),
            _ => bail!(
                "package `{name}` has several binaries ({}): set `default-run` in its Cargo.toml",
                bins.join(", ")
            ),
        };
        let workspace_root = metadata["workspace_root"].as_str().map(PathBuf::from).context("no workspace root")?;
        Ok(Self { package: name.to_owned(), bin, workspace_root })
    }
}

/// What a scan watches: the application's directories and files, and the
/// workspace root's entries.
pub(crate) fn roots(app: &Path, workspace: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = APP_DIRS.iter().chain(APP_FILES).map(|p| app.join(p)).collect();
    if workspace != app {
        out.extend(WORKSPACE_ENTRIES.iter().map(|p| workspace.join(p)));
    }
    out
}

/// A scan of `roots` under `app`. A directory that cannot be read keeps what
/// `previous` held under it (an unreadable directory is not a deletion);
/// its path is returned so the caller can warn.
pub(crate) fn snapshot(app: &Path, roots: &[PathBuf], previous: &Snapshot) -> (Snapshot, Vec<PathBuf>) {
    let mut out = Snapshot::new();
    let mut unreadable = Vec::new();
    for root in roots {
        match std::fs::metadata(root) {
            Ok(meta) if meta.is_dir() => walk(app, root, previous, &mut out, &mut unreadable),
            Ok(meta) => record(root, &meta, &mut out),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => keep(root, previous, &mut out, &mut unreadable),
        }
    }
    (out, unreadable)
}

fn walk(app: &Path, dir: &Path, previous: &Snapshot, out: &mut Snapshot, unreadable: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        keep(dir, previous, out, unreadable);
        return;
    };
    for entry in entries {
        // An entry that cannot be read: the directory is kept as it was.
        let Ok(entry) = entry else {
            keep(dir, previous, out, unreadable);
            continue;
        };
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Ok(kind) = entry.file_type() else {
            keep(&path, previous, out, unreadable);
            continue;
        };
        if kind.is_dir() {
            let relative = path.strip_prefix(app).unwrap_or(&path);
            let skipped =
                SKIPPED_DIRS.contains(&name.as_ref()) || SKIPPED_PATHS.iter().any(|p| relative == Path::new(p));
            if !skipped {
                walk(app, &path, previous, out, unreadable);
            }
        } else if !editor_temporary(&name) {
            match entry.metadata() {
                Ok(meta) => record(&path, &meta, out),
                Err(_) => keep(&path, previous, out, unreadable),
            }
        }
    }
}

fn record(path: &Path, meta: &std::fs::Metadata, out: &mut Snapshot) {
    out.insert(path.to_path_buf(), (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len()));
}

/// What `previous` held at or under `path`, kept as it was.
fn keep(path: &Path, previous: &Snapshot, out: &mut Snapshot, unreadable: &mut Vec<PathBuf>) {
    if !unreadable.iter().any(|p| p == path) {
        unreadable.push(path.to_path_buf());
    }
    out.extend(
        previous.range(path.to_path_buf()..).take_while(|(p, _)| p.starts_with(path)).map(|(p, s)| (p.clone(), *s)),
    );
}

/// Files editors write while saving (`.swp`, `4913`, `~` backups…): never a change.
pub(crate) fn editor_temporary(name: &str) -> bool {
    name.ends_with('~')
        || name.starts_with(".#")
        || name.ends_with(".swp")
        || name.ends_with(".swx")
        || name.ends_with(".tmp")
        || name == "4913"
        || name == ".DS_Store"
}

/// The files added, changed or removed between two scans.
pub(crate) fn changed<'a>(before: &'a Snapshot, after: &'a Snapshot) -> Vec<&'a PathBuf> {
    let mut out: Vec<&PathBuf> = after.iter().filter(|(p, s)| before.get(*p) != Some(*s)).map(|(p, _)| p).collect();
    out.extend(before.keys().filter(|p| !after.contains_key(*p)));
    out.sort();
    out.dedup();
    out
}

/// One line naming what changed: the first file and how many others.
pub(crate) fn describe(app: &Path, changes: &[&PathBuf]) -> String {
    let Some(first) = changes.first() else { return String::new() };
    let first = first.strip_prefix(app).unwrap_or(first).display().to_string();
    match changes.len() {
        1 => first,
        n => format!("{first} (+{})", n - 1),
    }
}

/// Whether `path` is an extension's source: `npm run build` turns it into
/// what the application embeds, so it never rebuilds the application itself.
pub(crate) fn extension_source(app: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(app) else { return false };
    EXTENSION_SOURCES.iter().any(|s| relative.starts_with(s))
}

/// Whether a manifest changed (the package or binary served may have changed).
pub(crate) fn manifest_changed(changes: &[&PathBuf]) -> bool {
    changes.iter().any(|p| p.file_name().is_some_and(|n| n == "Cargo.toml"))
}

/// Whether a migration changed (the server does not run them: `rustcms migrate`).
pub(crate) fn migrations_changed(app: &Path, changes: &[&PathBuf]) -> bool {
    let migrations = app.join("src/database/migrations");
    changes.iter().any(|p| p.starts_with(&migrations))
}

#[cfg(unix)]
mod unix {
    use std::process::{Child, Command, ExitCode, Stdio};
    use std::time::{Duration, Instant};

    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;
    use std::os::unix::process::CommandExt;
    use tokio::signal::unix::{SignalKind, signal};

    use super::*;

    /// A child running in its own process group (`process_group(0)`: the
    /// group's id is the child's pid), never reading the terminal.
    struct Group {
        child: Child,
    }

    impl Group {
        fn spawn(mut command: Command) -> std::io::Result<Self> {
            command.stdin(Stdio::null()).process_group(0);
            command.spawn().map(|child| Self { child })
        }

        fn pgid(&self) -> Pid {
            Pid::from_raw(self.child.id() as i32)
        }

        /// The leader's exit status once it exited.
        fn exited(&mut self) -> Option<std::process::ExitStatus> {
            self.child.try_wait().ok().flatten()
        }

        /// Whether a process of the group is still running (the leader may
        /// have exited while others went on).
        fn alive(&mut self) -> bool {
            let _ = self.exited();
            killpg(self.pgid(), None).is_ok()
        }
    }

    /// The signals that stop the watcher.
    struct Stop {
        interrupt: tokio::signal::unix::Signal,
        terminate: tokio::signal::unix::Signal,
    }

    impl Stop {
        fn new() -> std::io::Result<Self> {
            Ok(Self { interrupt: signal(SignalKind::interrupt())?, terminate: signal(SignalKind::terminate())? })
        }

        async fn received(&mut self) {
            tokio::select! {
                _ = self.interrupt.recv() => {}
                _ = self.terminate.recv() => {}
            }
        }
    }

    /// Stops `groups` together: SIGTERM, then SIGKILL to what is left after
    /// [`GRACE`], then reaps the leaders. Safe to run again on the same
    /// groups (after being interrupted).
    async fn stop_groups(groups: &mut Vec<Group>) {
        for group in groups.iter() {
            let _ = killpg(group.pgid(), Signal::SIGTERM);
        }
        let deadline = Instant::now() + GRACE;
        while groups.iter_mut().any(Group::alive) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        for group in groups.iter_mut() {
            if group.alive() {
                let _ = killpg(group.pgid(), Signal::SIGKILL);
            }
            let _ = group.child.wait();
        }
        groups.clear();
    }

    /// The watcher's state: the server (if running) and what it serves.
    struct Watch<'a> {
        project: &'a Project,
        target: Target,
        cargo: String,
        args: &'a [String],
        roots: Vec<PathBuf>,
        server: Option<Group>,
        signals: Stop,
        /// A stop was asked: no server is started any more.
        stopping: bool,
    }

    enum Built {
        Ok(Duration),
        Failed,
        Stopped,
    }

    impl Watch<'_> {
        fn cargo(&self) -> Command {
            let mut command = Command::new(&self.cargo);
            command.current_dir(&self.project.root);
            command
        }

        /// Waits for `group` (its output read by the caller), or a stop.
        async fn finish(&mut self, group: Group) -> Option<std::process::Output> {
            let mut groups = vec![group];
            loop {
                if groups[0].exited().is_some() {
                    let group = groups.pop().expect("a group");
                    return group.child.wait_with_output().ok();
                }
                tokio::select! {
                    () = self.signals.received() => {
                        self.stopping = true;
                        groups.extend(self.server.take());
                        stop_groups(&mut groups).await;
                        return None;
                    }
                    () = tokio::time::sleep(Duration::from_millis(50)) => {}
                }
            }
        }

        /// The package and binary to serve, from `cargo metadata` (a stop meanwhile: `None`).
        async fn resolve(&mut self) -> anyhow::Result<Option<Target>> {
            // Output into files, not pipes: a workspace's metadata outgrows a
            // pipe's buffer, and cargo would wait for a reader while we wait for it.
            let mut stdout = tempfile::tempfile().context("a temporary file")?;
            let mut stderr = tempfile::tempfile().context("a temporary file")?;
            let mut command = self.cargo();
            command
                .args(["metadata", "--no-deps", "--format-version", "1"])
                .stdout(stdout.try_clone()?)
                .stderr(stderr.try_clone()?);
            let group = Group::spawn(command).context("cannot run cargo")?;
            let Some(output) = self.finish(group).await else { return Ok(None) };
            let read = |file: &mut std::fs::File| -> std::io::Result<Vec<u8>> {
                use std::io::{Read, Seek};
                let mut out = Vec::new();
                file.rewind()?;
                file.read_to_end(&mut out)?;
                Ok(out)
            };
            if !output.status.success() {
                bail!("cargo metadata failed:\n{}", String::from_utf8_lossy(&read(&mut stderr)?));
            }
            let metadata: serde_json::Value =
                serde_json::from_slice(&read(&mut stdout)?).context("cargo metadata output")?;
            Target::from_metadata(&metadata, &self.project.root.join("Cargo.toml")).map(Some)
        }

        /// `cargo build` of the binary, the server (if any) running meanwhile.
        async fn build(&mut self) -> anyhow::Result<Built> {
            let mut command = self.cargo();
            command.args(["build", "--package", &self.target.package, "--bin", &self.target.bin]);
            let started = Instant::now();
            let mut build = Group::spawn(command).context("cannot run cargo")?;
            loop {
                if let Some(status) = build.exited() {
                    // A build script may have left processes in its group.
                    let mut rest = vec![build];
                    if rest[0].alive() {
                        self.stop_watching(rest).await;
                    } else {
                        let _ = rest[0].child.wait();
                    }
                    return Ok(if status.success() { Built::Ok(started.elapsed()) } else { Built::Failed });
                }
                tokio::select! {
                    () = self.signals.received() => {
                        self.stopping = true;
                        let mut groups = vec![build];
                        groups.extend(self.server.take());
                        stop_groups(&mut groups).await;
                        return Ok(Built::Stopped);
                    }
                    () = tokio::time::sleep(Duration::from_millis(100)) => {}
                }
            }
        }

        /// Stops `groups`; a stop asked meanwhile is kept (and the server goes too).
        async fn stop_watching(&mut self, mut groups: Vec<Group>) {
            let asked = tokio::select! {
                () = stop_groups(&mut groups) => false,
                () = self.signals.received() => true,
            };
            if asked {
                self.stopping = true;
                groups.extend(self.server.take());
                stop_groups(&mut groups).await;
            }
        }

        /// Stops the running server (as on a deploy); a stop asked meanwhile is kept.
        async fn stop_server(&mut self) {
            let groups: Vec<Group> = self.server.take().into_iter().collect();
            if !groups.is_empty() {
                self.stop_watching(groups).await;
            }
        }

        /// Starts the server through `cargo run`, as `rustcms serve` does,
        /// unless a stop was asked.
        fn start(&mut self) {
            if self.stopping {
                return;
            }
            let mut command = self.cargo();
            command
                .args(["run", "--quiet", "--package", &self.target.package, "--bin", &self.target.bin, "--", "serve"])
                .args(self.args);
            match Group::spawn(command) {
                Ok(group) => self.server = Some(group),
                Err(err) => eprintln!("watch: cannot start the server: {err}"),
            }
        }

        fn scan(&self, previous: &Snapshot) -> Snapshot {
            let (now, unreadable) = snapshot(&self.project.root, &self.roots, previous);
            for path in unreadable {
                eprintln!("watch: cannot read {} (kept as it was)", path.display());
            }
            now
        }

        /// Waits until a scan differs from `seen`, then until the files are
        /// quiet; the server's exit is reported meanwhile. `None` once stopped.
        async fn changes(&mut self, seen: &Snapshot) -> Option<Snapshot> {
            loop {
                if let Some(server) = self.server.as_mut()
                    && let Some(status) = server.exited()
                {
                    let mut groups: Vec<Group> = self.server.take().into_iter().collect();
                    stop_groups(&mut groups).await;
                    println!("watch: the server stopped ({status}); waiting for a change");
                }
                tokio::select! {
                    () = self.signals.received() => { self.stopping = true; return None; }
                    () = tokio::time::sleep(POLL) => {}
                }
                let mut now = self.scan(seen);
                if now == *seen {
                    continue;
                }
                // Quiet: two scans in a row agree.
                loop {
                    tokio::select! {
                        () = self.signals.received() => { self.stopping = true; return None; }
                        () = tokio::time::sleep(SETTLE) => {}
                    }
                    let again = self.scan(&now);
                    if again == now {
                        return Some(now);
                    }
                    now = again;
                }
            }
        }

        /// The watch loop; the caller stops the server whatever it returns.
        async fn rounds(&mut self) -> anyhow::Result<()> {
            let app = self.project.root.clone();
            // What the last build saw: changes after it are never lost, they
            // make the next round (at worst one more build that changes nothing).
            let mut seen = self.scan(&Snapshot::new());
            let mut changes: Vec<PathBuf> = Vec::new();
            loop {
                let before_build = seen.clone();
                match self.build().await? {
                    Built::Stopped => return Ok(()),
                    Built::Failed => {
                        let kept =
                            if self.server.is_some() { "the server keeps running" } else { "no server is running" };
                        println!("watch: build failed, {kept}; waiting for a change");
                    }
                    Built::Ok(took) => {
                        // Changed while building: build again before replacing.
                        let now = self.scan(&before_build);
                        if now != before_build {
                            let manifest = manifest_changed(&changed(&before_build, &now));
                            seen = now;
                            if manifest && !self.reresolve(&app).await {
                                if self.stopping {
                                    return Ok(());
                                }
                                (seen, changes) = self.wait_for_build(&app, seen).await?;
                            }
                            continue;
                        }
                        if self.stopping {
                            return Ok(());
                        }
                        self.stop_server().await;
                        if self.stopping {
                            return Ok(());
                        }
                        self.start();
                        let what = describe(&app, &changes.iter().collect::<Vec<_>>());
                        let what = if what.is_empty() { "started".to_owned() } else { format!("{what} → restarted") };
                        println!("watch: built in {:.1}s, {what}", took.as_secs_f64());
                    }
                }
                (seen, changes) = self.wait_for_build(&app, seen).await?;
                if self.stopping {
                    return Ok(());
                }
            }
        }

        /// The package and binary served, read again after a manifest changed;
        /// `false` when it cannot be (an invalid manifest: reported, the server
        /// keeps running) or a stop was asked.
        async fn reresolve(&mut self, app: &Path) -> bool {
            match self.resolve().await {
                Ok(Some(target)) => {
                    self.roots = roots(app, &target.workspace_root);
                    self.target = target;
                    true
                }
                Ok(None) => false,
                Err(err) => {
                    eprintln!("watch: {err:#}");
                    let kept = if self.server.is_some() { "the server keeps running" } else { "no server is running" };
                    println!("watch: cannot read the manifest, {kept}; waiting for a change");
                    false
                }
            }
        }

        /// Waits for changes that need a build (extension sources alone only
        /// get a reminder); the snapshot to build from and what changed. A
        /// stop: `stopping`.
        async fn wait_for_build(&mut self, app: &Path, mut seen: Snapshot) -> anyhow::Result<(Snapshot, Vec<PathBuf>)> {
            loop {
                let Some(now) = self.changes(&seen).await else { return Ok((seen, Vec::new())) };
                let found = changed(&seen, &now);
                if !found.is_empty() && found.iter().all(|p| extension_source(app, p)) {
                    println!("watch: extension sources changed; run `npm run build` in resources/admin");
                    seen = now;
                    continue;
                }
                if migrations_changed(app, &found) {
                    println!("watch: a migration changed; run `rustcms migrate`");
                }
                let manifest = manifest_changed(&found);
                let found: Vec<PathBuf> = found.into_iter().cloned().collect();
                println!("watch: {} changed, building", describe(app, &found.iter().collect::<Vec<_>>()));
                if manifest && !self.reresolve(app).await {
                    if self.stopping {
                        return Ok((now, found));
                    }
                    seen = now;
                    continue;
                }
                return Ok((self.scan(&now), found));
            }
        }
    }

    pub(super) async fn watch(project: &Project, args: &[String]) -> anyhow::Result<ExitCode> {
        // Signals first: every child started afterwards is stopped with the watcher.
        let signals = Stop::new()?;
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let placeholder = Target {
            package: project.package.clone(),
            bin: project.package.clone(),
            workspace_root: project.root.clone(),
        };
        let mut state = Watch {
            project,
            roots: roots(&project.root, &project.root),
            target: placeholder,
            cargo,
            args,
            server: None,
            signals,
            stopping: false,
        };
        let Some(target) = state.resolve().await? else { return Ok(ExitCode::SUCCESS) };
        state.roots = roots(&project.root, &target.workspace_root);
        state.target = target;
        println!("watch: building `{}`; Ctrl-C to stop", state.target.bin);
        let result = state.rounds().await;
        // Whatever ended the loop (a stop, an error), the server goes with it.
        let mut groups: Vec<Group> = state.server.take().into_iter().collect();
        stop_groups(&mut groups).await;
        result.map(|()| ExitCode::SUCCESS)
    }
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
    fn watch_is_a_flag_of_serve_before_the_separator() {
        let args = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(requested(&args(&["--watch", "--api-only"])));
        assert!(!requested(&args(&["--", "--watch"])));
        assert_eq!(
            without_flag(&args(&["--watch", "--api-only", "--", "--watch"])),
            args(&["--api-only", "--", "--watch"])
        );
    }

    #[test]
    fn scans_sources_and_configuration_but_not_templates_or_output() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for path in [
            "src/main.rs",
            "src/database/migrations/m1.rs",
            "config/app.toml",
            "lang/vi.toml",
            "resources/admin/js/x.js",
            "resources/views/admin/page.html",
            "resources/admin/node_modules/x/index.js",
            "target/debug/app",
            "storage/media/a.png",
            "src/.main.rs.swp",
            "src/main.rs~",
            ".env",
            "Cargo.toml",
        ] {
            write(root, path, "x");
        }
        let (found, unreadable) = snapshot(root, &roots(root, root), &Snapshot::new());
        assert!(unreadable.is_empty());
        let mut names: Vec<String> =
            found.keys().map(|p| p.strip_prefix(root).unwrap().display().to_string()).collect();
        names.sort();
        assert_eq!(
            names,
            [
                ".env",
                "Cargo.toml",
                "config/app.toml",
                "lang/vi.toml",
                "resources/admin/js/x.js",
                "src/database/migrations/m1.rs",
                "src/main.rs"
            ]
        );
    }

    #[test]
    fn extension_sources_and_manifests_are_told_apart() {
        let app = Path::new("/app");
        assert!(extension_source(app, &app.join("resources/admin/src/acme/widget.tsx")));
        assert!(extension_source(app, &app.join("resources/admin/package.json")));
        assert!(!extension_source(app, &app.join("resources/admin/js/acme/widget.js")));
        assert!(!extension_source(app, &app.join("resources/admin/css/acme/widget.css")));
        // Other files there are embedded as they are (a logo): they rebuild.
        assert!(!extension_source(app, &app.join("resources/admin/logo.png")));
        assert!(!extension_source(app, &app.join("src/main.rs")));
        let manifest = app.join("Cargo.toml");
        let main = app.join("src/main.rs");
        assert!(manifest_changed(&[&main, &manifest]));
        assert!(!manifest_changed(&[&main]));
    }

    #[test]
    fn the_workspace_root_adds_its_manifest_lockfile_and_cargo_config() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        let app = workspace.join("apps/blog");
        write(&app, "src/main.rs", "x");
        write(workspace, "Cargo.lock", "x");
        write(workspace, ".cargo/config.toml", "x");
        write(workspace, "other/src/lib.rs", "x");
        let (found, _) = snapshot(&app, &roots(&app, workspace), &Snapshot::new());
        assert!(found.contains_key(&workspace.join("Cargo.lock")));
        assert!(found.contains_key(&workspace.join(".cargo/config.toml")));
        assert!(!found.contains_key(&workspace.join("other/src/lib.rs")));
    }

    #[test]
    fn reports_added_changed_and_removed_files_and_migrations() {
        let at = |s| (SystemTime::UNIX_EPOCH + Duration::from_secs(s), 1);
        let root = Path::new("/app");
        let before: Snapshot = [(root.join("src/a.rs"), at(1)), (root.join("src/b.rs"), at(1))].into_iter().collect();
        let after: Snapshot =
            [(root.join("src/a.rs"), at(2)), (root.join("src/database/migrations/m2.rs"), at(1))].into_iter().collect();
        let found = changed(&before, &after);
        assert_eq!(found.len(), 3);
        assert!(migrations_changed(root, &found));
        assert_eq!(describe(root, &found), "src/a.rs (+2)");
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_directory_is_not_a_deletion() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "src/admin/x.rs", "x");
        let (first, _) = snapshot(root, &roots(root, root), &Snapshot::new());
        let locked = root.join("src/admin");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let (second, unreadable) = snapshot(root, &roots(root, root), &first);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Running as root reads anything: nothing to check then.
        if !unreadable.is_empty() {
            assert_eq!(unreadable, [locked]);
            assert_eq!(first, second, "kept as it was");
        }
    }

    #[test]
    fn the_binary_is_the_package_s_only_one_or_its_default_run() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("Cargo.toml");
        std::fs::write(&manifest, "").unwrap();
        let metadata = |bins: &[&str], default: Option<&str>| {
            serde_json::json!({
                "workspace_root": dir.path(),
                "packages": [{
                    "name": "blog",
                    "manifest_path": manifest,
                    "default_run": default,
                    "targets": bins.iter().map(|b| serde_json::json!({"name": b, "kind": ["bin"]}))
                        .chain([serde_json::json!({"name": "blog", "kind": ["lib"]})]).collect::<Vec<_>>(),
                }]
            })
        };
        let target = Target::from_metadata(&metadata(&["server"], None), &manifest).unwrap();
        assert_eq!((target.package.as_str(), target.bin.as_str()), ("blog", "server"));
        assert_eq!(Target::from_metadata(&metadata(&["a", "b"], Some("b")), &manifest).unwrap().bin, "b");
        let several = Target::from_metadata(&metadata(&["a", "b"], None), &manifest).unwrap_err();
        assert!(several.to_string().contains("default-run"), "{several}");
        assert!(Target::from_metadata(&metadata(&[], None), &manifest).is_err());
    }
}
