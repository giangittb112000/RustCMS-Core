# Several instances

One instance needs nothing more. This page is for running several behind a
load balancer: what they must share, what Redis adds, and what stays per
instance.

## Quick start

1. Build with the `redis` feature and set `REDIS_URL` (`[redis] url`): one
   standalone Redis (Redis Cluster is not supported).
2. Give every instance the same `APP_KEY`, `DATABASE_URL` and `redis.prefix`;
   two applications sharing a Redis need different prefixes (default
   `rustcms:<app.name>`).
3. Share uploads: S3 (feature `s3`), or one volume mounted by every instance
   ([Storage](storage.md)).
4. Run `migrate` once per deploy, before the new instances start.
5. Keep the clocks synchronized (NTP): the queue and the scheduler compare
   times written by different instances.

```toml
# config/redis.toml (or REDIS_URL / RUSTCMS_REDIS__PREFIX); the file is the [redis] section
url = "redis://redis.example.com:6379/0"
prefix = "rustcms:blog"
```

## API mode

An instance in API mode serves the application API only: your `ApiRoutes`,
the API guard's auth routes (`/api/v1/auth/token`, `…/jwt`, `…/refresh`,
`…/me`) and the routes you mount yourself. Scale these out behind a load
balancer; keep at least one instance in `full` mode for the admin panel.

```sh
rustcms serve --api-only        # or [app] mode = "api" / RUSTCMS_APP__MODE=api
```

- **Left out:** the admin panel and its assets, the admin API, the panel's
  sign-in routes, and every [`AdminRoutes`](../admin/admin-routes.md) route
  (streams and sockets too). Every request under `auth.admin_path` answers
  a JSON 404 before routing, whatever its method, and no session is loaded.
- **Declared as in `full`:** permissions, job kinds, schedules, commands and
  encrypted columns, so every instance agrees (`permission:sync` from any
  of them, jobs enqueued by one and run by another).
- **Background work off by default:** `queue.enabled` and `schedule.enabled`
  left out are `false` in API mode; set them to `true` to run workers or
  the scheduler there. Each instance still sweeps the files it staged.
- **Your own code:** check `state.mode()` (or `app.mode()` in a provider)
  before mounting panel-only routes with `app.routes` or starting services
  only the panel uses.
- `serve` in API mode logs a warning for what several instances need and
  this one lacks: a SQLite database, no Redis (rate limits per instance), a
  `local` storage disk (one shared volume, or `s3`).
- `route:list` lists the routes of the mode it runs in:
  `RUSTCMS_APP__MODE=api rustcms route:list`.

A typical deployment:

| Instances | Command | Serves |
|---|---|---|
| 1 | `rustcms serve` | the admin panel (the load balancer sends `auth.admin_path` here), the scheduler |
| N | `rustcms serve --api-only` | the API, no sticky sessions needed |
| M | `rustcms queue:work` | queued jobs (mail, exports, imports, yours) |

The instances run the same build with different configuration. An API
instance's configuration may add the opt-in caches of
[Credential revocation](../security/revocation.md#several-instances) when it
serves heavy authenticated traffic and a second's delay for revocations made
elsewhere is acceptable:

```toml
# config/app.toml on API instances (or RUSTCMS_APP__MODE=api)
mode = "api"

# config/auth.toml on API instances: optional
permission_generation_cache_ms = 1000

[jwt]
session_cache_ms = 1000
```

All of them share `APP_KEY` (and `APP_PREVIOUS_KEYS`), `DATABASE_URL`, the
storage, `redis.prefix`, `http.trusted_proxies` (the load balancer's
addresses) and `APP_URL`: links in mails are built from the `APP_URL` of the
instance that runs the job, so it must reach a `full` instance. Size
`database.max_connections` so that all instances together stay under the
database's limit. Health checks use `GET /health` (it pings the database).

### Before you scale out

API mode removes the panel; what a public API needs beyond that is the
application's to decide. Check these before putting API instances in front of
a site:

- **Who may read what.** Your API routes filter drafts, scheduled dates and
  trashed records themselves: a resource's `scope()` and policy protect the
  panel's operations, not the queries your handlers write.
- **Files.** The only built-in file route is the panel's (signed-in users,
  under `auth.admin_path`, refused in API mode). Serve the files your readers
  may see from an API route of your own (`rustcms::storage::of(&state)`),
  with your rules for which ones and how long they are cached
  ([Headless rendering](../admin/media.md#headless-rendering)).
- **Browsers calling the API.** There is no CORS layer: a frontend server
  calling the API needs none; for browsers on another origin add one with
  `app.middleware` (or at the proxy), with the origins you allow.
- **The reader's address.** When a frontend server calls the API for its
  readers, every request comes from that server: per-IP limits would count
  all readers together. List the frontend servers in `http.trusted_proxies`
  and have them send `X-Forwarded-For`.
- **Fresh pages.** A frontend that caches pages learns about changes only if
  you tell it: dispatch a job from your resources' hooks (and
  [media hooks](../admin/media.md#media-hooks)) that calls it.
- **Paging.** `ApiPage` pages by number: rows added or removed while a reader
  pages may repeat or be skipped. Page by key (`id > last`) where that
  matters.
- **Releases.** Every instance runs the same build. Run `migrate` once,
  before the new instances start, with migrations that only add, so the
  instances still running the previous build keep working.

## Redis configuration

| Key | Default | |
|---|---|---|
| `url` (`REDIS_URL`) | unset: no Redis | `redis://host:port/db`; empty counts as unset |
| `prefix` | `rustcms:<app.name>` | starts every key of this deployment; may not be empty or contain whitespace |

- Boot fails when Redis cannot be reached, when the prefix is invalid, or
  when `url` is set but the application was built without the `redis`
  feature.
- Commands time out after a second, connecting after two.

## What Redis is used for

- **Rate limits** (sign-in, `Throttle<N>`, uploads, …) are counted in Redis
  for all instances, atomically across the keys of one check. If Redis does
  not answer, checks fail closed (500).

The cleanups need no Redis. The auth, idempotency, media and exports
cleanups are [framework schedules](scheduling.md#framework-schedules)
(`rustcms.auth-prune`, `rustcms.idempotency-prune`, `rustcms.media-prune.<disk>`,
`rustcms.exports-prune.<disk>`): each slot is claimed once in the database and run
only by `serve` with the scheduler. Files an instance stages on its own disk
(uploads, exports and imports being written) are swept by that instance:
every `serve` and `queue:work` removes its abandoned ones, older than an hour,
every 10 minutes. See [Data retention](../operations/retention.md).

Permissions need no Redis: a change made on one instance applies on every
instance from the next request (the permission cache follows a counter kept
in the database). API instances under heavy load may reuse that read, and a
JWT's sign-in, for a short time with `auth.permission_generation_cache_ms`
and `auth.jwt.session_cache_ms` (off by default; changes then reach them
within that time: [Credential revocation](../security/revocation.md#several-instances)).

The queue and the scheduler need no Redis either: they coordinate through
the database (see [below](#your-own-periodic-jobs)).

## What stays per instance

- Without Redis, several instances still work, but each keeps its own rate
  limits: run one instance unless Redis is configured.
- Password-hashing and upload concurrency limits are always per instance
  (each bounds its own CPU and disk).
- [Runtime branding](../admin/customizing.md#runtime-branding) is cached per
  instance: `AdminBranding::invalidate()` clears this instance only; the
  others load it again once their `[admin] branding_ttl_secs` has passed.
  Redis does not broadcast it.
- [Realtime events](../admin/realtime.md) are per instance too. Each instance
  runs its own hub producers, and the `[realtime]` connection limits apply to
  each one. A change on one instance still reaches every open stream within a
  recheck, because rechecks read the database.

## The queue on several instances

Every instance may run [queue workers](queues.md#several-instances): a job
is claimed by one worker at a time, with a lease of `queue.lease_secs` (60)
renewed while it runs. A lease counts until `queue.clock_skew_secs` (5) after
it ends, so keep clock differences below that. Run workers in their own
processes with `rustcms queue:work` and set `queue.enabled = false` on web
instances if you want to scale them apart ([Running the queue](../operations/queue.md)).

## Your own periodic jobs

Use a [schedule](scheduling.md). The demo declares its own from the
bootstrap:

<<< @/../../../examples/demo/src/jobs/post_digest.rs#schedule [src/jobs/post_digest.rs]

Schedules run only in `serve`. Each slot is claimed once in the database
across instances, with or without Redis, and enqueues the job (or runs the
task); the job must still be safe to retry. Turn the scheduler off on
instances that should not run it with `[schedule] enabled = false`.

`rustcms::jobs::exclusive(&state, name, ttl, f)` is a separate, best-effort
helper. It runs `f` unless another instance holds a Redis lease named `name`
(held at most `ttl`), and returns whether this instance ran it. Without
Redis it always runs. If Redis does not answer, `f` runs anyway, and a lease
can expire during a long run: write work that is correct when two runs
overlap (claim rows in the database), the lease only saves work. Prefer a
schedule for anything periodic.

## Related

- [Scheduling](scheduling.md)
- [Queues](queues.md)
- [Storage](storage.md)
- [Production checklist](../security/production.md)
- [Configuration](../getting-started/configuration.md)
