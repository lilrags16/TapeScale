# Security and media-safety model

TapeScale protects offline recovery data, credentials, encryption keys, and destructive hardware. Security failures can destroy the final usable copy, so the system defaults to denial when identity or state is uncertain.

This document is the required production security contract. The alpha implements core RBAC/destructive-action policy, overwrite denial, restore-path validation, simulator claim boundaries, and a constrained external-tool runner. It does not yet constitute a production authentication, MFA, encrypted-secret, TLS, key-management, or device-security implementation. Those requirements remain production release blockers even where the UI demonstrates the intended workflow.

## Threat model

The design considers:

- a compromised browser session or insufficiently authorized operator;
- malicious source filenames, archive paths, link targets, metadata, and native properties;
- lost, foreign, mislabeled, or unexpectedly loaded media;
- stale catalogs, replayed events, worker crashes, device resets, and split ownership;
- a compromised source NAS and loss of the original application database;
- secrets leaking through logs, diagnostics, notifications, environment variables, or exports;
- shell metacharacters and hostile values passed to external utilities;
- an attacker attempting to reduce retention, relabel, erase, export keys, or overwrite live destinations.

## Trust boundaries

- Browser and API inputs are untrusted, even for administrators.
- Tape labels, barcodes, engine output, manifests recovered from media, and archive paths are untrusted until authenticated and reconciled.
- The API service cannot access devices or writable source/recovery data. The alpha stack is an explicit exception: it gives the API read-only `/source` and recovery-artifact views plus `DAC_READ_SEARCH` for contained metadata browsing and preflight. It must never return file contents, absolute paths, symlink targets, or artifact storage references.
- The privileged worker receives only explicit intents already authorized by the API, then independently rechecks policy, object versions, lease ownership, and current physical state.
- PostgreSQL is durable product state, not proof that a physical write completed.
- Engine state is authoritative for its records but does not override TapeScale retention or authorization.

## Identity and authorization

Default roles are:

- Administrator: configure hardware, policies, identities, keys, updates, retention, and destructive actions.
- Operator: run permitted jobs, satisfy media requests, confirm assigned movements, and restore to approved destinations.
- Viewer: inspect authorized status, inventory, verification evidence, and coverage.

The API and worker enforce the same permissions. Unique accounts and protected sessions are required. Administrators enroll MFA by default and receive one-time recovery codes. No image ships a shared default password. Session cookies must be `Secure`, `HttpOnly`, and `SameSite`; state-changing browser requests require origin/CSRF protection. Login, MFA, and reauthentication endpoints are rate-limited.

Erase, relabel, retention reduction, key export, destruction of protection, and overwrite of existing data require:

1. an administrator role;
2. a server-generated preview listing every affected recovery point, dependency, tape, and destination;
3. recent MFA-backed reauthentication;
4. an execution token bound to the preview digest and current object versions;
5. an immutable audit result.

## Media safety

Media is write-protected in policy unless every overwrite-gate condition in [ARCHITECTURE.md](ARCHITECTURE.md) passes. Additional rules are:

- Unknown or foreign media begins protected.
- A barcode or printed label is never sufficient identity.
- Labeling, format import, erasing, and reuse are separate operations.
- A reset or disconnect makes position and pending writes uncertain.
- Off-site return changes custody only; it does not change retention.
- WORM status cannot be overridden by software.
- Exported or active-job media remains reserved against conflicting writes.
- Destructive exceptional overrides cannot suppress the resulting protection gap or audit event.

The alpha's optional rewind/unload path is an isolated-lab exception, not the production authorization model. It requires an exact reconciled drive, known zero-in-flight kernel state, a durable fenced lease, and an explicit designated-scratch-media acknowledgement. Linux permits these two endpoint operations to recover from its internal lost-position state; this does not authorize data I/O or relative positioning. Linux can finalize another controller's prior write with a filemark before motion, so the control must never be used with retained, production, unknown, or foreign media. The alpha API has no accounts yet; when the lab flag is enabled, its network must be isolated and attended.

The optional `btape` path is a more destructive isolated-lab exception. Queueing requires two independent boolean acknowledgements and the exact phrase `DESTROY <registered-label> WITH BACULA BTAPE TEST`. The selected inventory record must be non-WORM, unheld, content-free, unreserved, and neither foreign nor mismatched; this remains an operator assertion because a standalone drive cannot prove the printed inventory label. The worker rechecks the exact device identity and fenced reservation, accepts only the pinned `/usr/sbin/btape` binary/digest and fixed `test\n` transcript, requires completed-test output markers, and rewinds to an observed BOT before recording a pass. It never promotes the broader qualification state. The alpha has no authentication, so port 18080 is an unauthenticated destructive surface: deploy it only on an isolated attended lab network, disable `TAPESCALE_LAB_TAPE_QUALIFICATION_ENABLED` whenever the test is not actively required, and never load non-scratch media while it is enabled.

The **Prepare tape** wizard does not combine these checks into one authority-bearing request. It sequences drive reconciliation, cartridge registration or exact-record reuse, and qualification through their existing API contracts. It may auto-select only one current safe drive; it cannot infer which physical cartridge is loaded, mark WORM media scratch, suppress holds/content/reservations, or dispatch a tape command before both destructive booleans and the exact phrase are accepted. Each wizard qualification submission carries a browser-generated idempotency key. PostgreSQL serializes that key, fingerprints the exact drive/media request, returns the original current or terminal run on a matching replay, and rejects changed-payload reuse. An ambiguous network retry therefore cannot silently create a second destructive run.

The source and recovery-manifest browsers expand filename visibility on the unauthenticated test API. Both are GET-only, non-cacheable, one-level, bounded metadata projections. A directory larger than the display bound is returned with `truncated=true` and must be presented as partial; the backup and retained manifest themselves are not truncated. Source browsing holds the read-only root/selection as descriptors, resolves beneath them without following symlinks, and obtains entry metadata relative to the held directory; special objects remain non-navigable. Recovery browsing first verifies the retained manifest artifact and then projects its recorded entries without exposing the artifact path or reading source/tape content. This disclosure is another reason to keep port 18080 on an isolated attended network. Browser output is advisory. For a selected restore, plan creation validates every requested path against the authenticated manifest, recursively expands directories, closes hard-link groups, removes redundant descendants, and snapshots a deterministic normalized scope/hash. Execution is offered only for an exactly one-volume point; a multi-volume selected scope is blocked until TapeScale can prove path-to-media coverage and truthful per-volume reads. The worker reloads the manifest and requires the same result before tape access. Parameterized Bacula catalog lookups are pinned to the original JobId, and only numeric FileIndex/PathId values—not operator paths—enter restricted BVFS commands.

## External command execution

Rust integrations invoke programs with an executable path and an argument array. They must not use `sh -c`, interpolate user data into command strings, or inherit an uncontrolled environment.

The tool runner:

- allowlists executable and operation combinations;
- validates dataset, snapshot, device, and destination arguments;
- sets an explicit working directory, environment, uid/gid, and timeout where available;
- bounds captured output and redacts configured secrets;
- records executable version, argv classification, exit status, timeout, and correlation ID;
- treats a failed producer, consumer, or pipeline endpoint as failure;
- handles cancellation at operation-specific safe boundaries;
- turns missing tools and hung commands into actionable durable states.

Acceptance AT-30 includes shell metacharacters, failed producers, timeouts, cancellation, and missing dependencies.

The alpha's operational backup path is an isolated-lab exception to the production authorization and encryption contract. It accepts only a server-contained named `.zfs/snapshot/<name>` source on a kernel-reported read-only mount, one Full file job, one exact G0.1-qualified drive, and an ordered set of independently qualified and authorized scratch cartridges. The operator confirms the initially loaded identity, complete overwrite, and unencrypted deviation with a phrase bound to the cartridge label and request UUID. Every continuation is separately order-bound and requires the exact requested cartridge plus its own typed confirmation before the worker may label or write it. The worker independently rechecks source access, identity, qualification, compatibility, pool, holds, content, WORM, reservation, and readiness before each durable write boundary. The alpha still has no authentication, so anyone who can reach its API can attempt these visible confirmations. Do not place sensitive production data in this unencrypted slice or expose it outside the isolated attended lab network.

## Restore containment

Restored names and metadata are hostile. File restoration must anchor all resolution to an already approved destination and reject absolute paths, `..`, alternate separators, NULs, device paths, and platform-specific escape forms. Directory traversal checks alone are insufficient.

Creation uses descriptor-relative operations or an equivalent race-resistant mechanism that refuses unexpected symlinks. Symlink and hard-link targets are validated before creation; a hard link cannot target an inode outside the restore root. Special files require an explicit policy and isolated destination. Native receives rewrite mountpoints and dataset names into an approved restore hierarchy, do not mount over live paths, do not enable recovered shares, and restrict imported properties.

Existing destinations default to skip conflicts. Rename and replace are explicit. Dataset replacement and deletion reconciliation are destructive administrator operations with preview and reauthentication.

The alpha's attended verified-point rehearsal is a narrow laboratory exception, not the production authorization model. It requires an unexpired blocker-free plan and unchanged preview hash/restore scope, explicit exact-drive selection, three attended confirmations, and a typed phrase bound to the plan. The API cannot read worker evidence files or the restore destination. The worker independently revalidates the plan provenance, current drive/media reservation and fencing, original Bacula catalog/volume, retained manifest artifact, and complete-point or selected-path scope before dispatch. A complete-point run reserves the complete ordered cartridge set; a selected run is permitted only when the point has exactly one retained cartridge. API planning, queueing, claim, and worker gates independently reject a multi-volume selected run so an older persisted plan cannot bypass the boundary after upgrade. The worker creates a brand-new UUID destination and has no rehearsal code path for label, erase, BOT invalidation, backup, media reuse, existing-destination replacement, or publication. Internal artifact storage paths never enter public detail or evidence responses. A selected pass records `selective_restore_test` and cannot satisfy a complete-point `restore_test` gate. The alpha still has no authentication, so deploy this physical-read surface only on the isolated attended lab network described for G0.1, operational backup, and G0.2.

## Encryption and keys

Encryption is enabled by default for new policies and covers content plus sensitive filenames, paths, metadata, and exported catalogs. Use an established authenticated-encryption format or a qualified encryption-plus-authentication construction. Compression occurs before application encryption. Hardware encryption is optional; a qualified software path is required for older drives.

Key records refer to lineages and identifiers. Usable key material is encrypted at rest and never written to ordinary manifests, logs, or diagnostics. Historical keys remain protected while retained records depend on them.

Catalog recovery and key recovery are independent. A portable encrypted key package is stored away from the source NAS and application catalog, and its recovery is verified before media becomes ordinarily export-ready. Raw ZFS encryption can require original dataset keys and does not automatically conceal all outer metadata; the recovery plan lists these dependencies explicitly.

An administrator can opt out only with a recorded reason and visible deviation. Tape identity, record integrity, manifest authentication, truthful status, retention, and overwrite gates cannot be disabled.

## Secrets and transport

- Browser/API traffic uses HTTPS.
- TrueNAS control uses authenticated secure WebSocket JSON-RPC with certificate verification and negotiated supported version.
- Internal service communication is authenticated and protected; database and engine control ports remain private.
- Long-lived secrets are mounted from secret files or a platform secret provider, not committed configuration.
- Diagnostics use an allowlist and redact credentials, recovery codes, usable keys, cookies, authorization headers, and sensitive path inventories.
- Notifications contain the minimum operational context and link an authenticated user to details.

## Audit and event integrity

Security, media, custody, retention, verification, and destructive actions append audit records containing actor, authenticated method, correlation ID, object versions, request/preview digest, decision, result, and timestamp. Corrections append a superseding event and preserve the prior record.

Notifications use durable event identities. Replay may retry delivery but cannot silently duplicate movement confirmation or destructive execution.

The alpha database schema and domain rules establish this contract; full authenticated end-to-end audit/outbox delivery evidence remains pending in AT-28.

## Container hardening

- Run as non-root whenever device access permits; use explicit uid/gid and supplementary device groups.
- Drop all capabilities, then add only a documented minimum to the worker or constrained host helper.
- Prefer read-only root filesystems and writable named volumes for exact state paths.
- Mount source data read-only and approved restore destinations narrowly. If manifest browsing is enabled, expose only the exact recovery-artifact volume to the API as read-only; never mount the worker spool, Bacula runtime, or writable recovery state there.
- Do not mount `/`, the Docker socket, arbitrary `/dev`, or broad host configuration.
- Apply CPU, memory, process, and spool/staging limits.
- Generate an SBOM and scan the release image; pin base images and dependencies.

## Incident behavior

When identity, position, catalog agreement, key availability, or integrity is uncertain, TapeScale preserves media and recoverable state, blocks destructive continuation, raises an actionable condition, and retains evidence. Operators should not bypass this state at the engine console; follow reconciliation in [OPERATIONS.md](OPERATIONS.md).
