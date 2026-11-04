# 0012 — Several instances (Redis)

- Status: accepted
- Date: 2026-10-15

## Context

0.1 ran one instance: rate limits and the permission cache lived in memory,
and periodic jobs ran in every process. 0.2 supports several instances of one
application.

## Decision

- **Permissions need no Redis.** The cache is keyed by a counter in the
  database (`auth_generations.rbac`), advanced inside every RBAC transaction
  and read on every request: a committed change applies on every instance from
  its next request, an uncommitted one on none. A Redis-based invalidation
  could commit in the database and fail in Redis, serving stale permissions.
- **Rate limits** use Redis when configured (feature `redis`): one Lua script
  checks every key, then counts all of them (all-or-nothing, as in memory);
  windows start when their key is created and refunds never extend them or go
  below zero. If Redis does not answer, limits fail closed.
- **Periodic jobs** take a Redis lease (random owner token, compare-and-delete
  release) to reduce duplicate runs: runs can still overlap after a lease
  expires, and the job runs anyway when Redis does not answer. Leases only
  avoid duplicate work:
  every job stays correct when two run, e.g. media deletions are claimed with
  `FOR UPDATE SKIP LOCKED` on PostgreSQL and MySQL.
- **Scope:** one standalone Redis (no Cluster), one key prefix per deployment;
  connect and command timeouts bounded. Password-hashing and upload concurrency
  limits stay per instance (they bound that instance's CPU and disk).

## Consequences

- One extra primary-key read per request for the permission generation.
- Without Redis, several instances work but each keeps its own rate limits:
  run one instance unless Redis is configured.
- Media storage must be shared between instances (S3 or one volume).

## Amendment (Unreleased, 2026-11-04): opt-in reuse of two reads (ADR 0036)

Approved by the owner after the API benchmark
(`docs/core/reports/api-performance.vi.md`): the generation read is one of
the statements every authenticated request pays. Both settings are **off by
default** (owner's decision, 2026-11-04): the framework does not weaken every
project's revocation guarantee for the needs of one deployment; API
instances opt in (e.g. `1000`).

- `auth.permission_generation_cache_ms` (default 0, 0 to 60,000): each
  instance reuses the database's `auth_generations.rbac` it read for that
  long. A change made through an instance still applies there on the next
  request (its local generation moves after the commit); on the others it
  applies within the period. `0` restores "read on every request".
- `auth.jwt.session_cache_ms` (default 0, 0 to 60,000): a JWT's family and user are
  read together (one join) and reused for that long, keyed by the local
  generation, so `PermissionResolver::evict_all` forgets them (RBAC changes,
  and the sign-out, refresh-replay, password, logout-others, reset and
  invitation endpoints call it after their commit). Sign-outs and account
  changes made elsewhere apply within the period. A load that started before
  a local eviction is not kept.
- Sessions and API tokens are not cached: their revocation is never delayed
  (their permissions follow the generation above).
- The decision above ("a committed change applies on every instance from its
  next request") holds by default; with a period set, within that period. The
  JWT check reads family and user in one join in every case.

