//! Load-test application (`just bench-api`): one table, API routes without
//! throttles, and the admin panel with one resource (to compare `full` and
//! `api` modes).

mod app;

fn main() -> std::process::ExitCode {
    rustcms::Console::<app::Migrator>::new(app::build).run_blocking()
}
