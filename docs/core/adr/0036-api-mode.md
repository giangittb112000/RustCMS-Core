# ADR 0036: API mode

- Status: accepted (Unreleased)
- Date: 2026-11-04

## Context

The application API (`ApiRoutes`, `/api/v1/auth/*`) and the admin panel are
served by one binary and one process. Deployments that scale the API out
behind a load balancer want instances that serve the API only: no panel, no
sessions, no streams or sockets, no background work competing for the
database pool. Several instances already work (ADR 0012, 0016, 0031): rate
limits through Redis, the permission cache through a database counter, the
queue and the schedules through the database.

"Worker" already names queue workers (`queue:work`, `[queue] workers`), so
the mode is called API mode.

## Decision

- **`[app] mode`**: `full` (default) or `api`; `RUSTCMS_APP__MODE`, and
  `serve --api-only`, which sets it through `Config::with` before the
  application is built (so `section`/`with` keep it). `AppState::mode()` and
  `AppBuilder::mode()` tell providers and applications.
- **What API mode leaves out**: every route the panel mounts (the SPA and its
  assets, the admin API, settings, queue, schedules, audit, exports, imports,
  media, dashboard, custom pages and slots), the panel's auth routes
  (`{admin}/api/auth/*`), and every `AdminRoutes` route (streams and sockets
  included).
  - The core router refuses every path under a declared panel prefix
    (`AppBuilder::panel_path`, `AuthProvider` declares `auth.admin_path`) with
    a JSON 404, in front of the routes: before routing (no 405 or `Allow`
    for a route an application mounted there itself), before any body is
    read.
  - The auth guard does the same first, before its asset bypass: no session
    is ever loaded under the admin path of an API instance.
- **What it keeps**: everything other instances rely on is declared as in
  `full`: permissions (also the panel's, so `permission:sync` from any
  instance orphans nothing), job kinds, schedules, commands, encrypted
  columns, mail templates, the registry, the panel's services. Only routes,
  route descriptions, body limits of panel routes and the CSP differ.
  Applications check `mode()` for the routes they mount themselves and their
  own background services.
- **Background work**: `[queue] enabled` and `[schedule] enabled` left out
  follow the mode (on in `full`, off in `api`); a value set wins. The public
  `bool` fields are resolved in `QueueConfig::of` / `ScheduleConfig::of`.
  `queue:work` is unchanged.
- **Cleanups**: the media and exports cleanups became framework schedules
  (ADR 0031 amendment of 2026-11-04) and each instance sweeps its own staging
  directory from `serve` (any mode) and `queue:work`, because boot-time loops
  cannot know whether an instance stages files.

## Consequences

- One binary serves both roles; a load balancer sends the admin path to
  `full` instances and the rest anywhere. Mail links are built from the
  `app.url` of the instance running the job: every instance keeps the same
  `app.url`, the one that reaches a `full` instance.
- The refusal wraps the routes as one service (a `Router::layer` would run
  after the route and method are chosen, and a method router still adds its
  `Allow` header): an API instance answers the same 404 for every path and
  method under the admin path, before any body limit.
- API mode does not make API requests cheaper by itself (measured in
  `docs/core/reports/api-performance.vi.md`): the cost of an authenticated
  request is its SQL statements. The PAT check was reduced in the same work,
  and, with the owner's approval, the RBAC generation and JWT sign-in reads
  are reused for a second per instance (ADR 0012 amendment).
