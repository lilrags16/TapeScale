# TapeScale interface design system

Design direction: **quiet control room** — a dense, calm storage-operations console optimized for accurate decisions under routine and exceptional conditions. TapeScale should feel purpose-built for long-lived physical media, not like a generic SaaS dashboard.

## Product principles

1. Put active work, operator actions, and recovery coverage before decorative metrics.
2. Distinguish planned, observed, confirmed, written, verified, and restore-tested states in language as well as color.
3. Keep tape IDs, paths, dates, quantities, dependencies, and failure details legible.
4. Prefer compact searchable tables, timelines, checklists, and contextual detail panels over large card grids.
5. Show uncertainty and estimates explicitly. Never smooth a dangerous state into a reassuring summary.
6. Make the safe action easiest without hiding exceptional administrator paths.

## Visual identity

The visual metaphor is archival equipment: warm paper neutrals around a graphite console, precise rules, monospaced identifiers, and restrained signal colors. Avoid gradients, glass effects, ornamental charts, giant hero copy, excessive rounding, and empty display-card layouts.

### Color tokens

| Token | Suggested value | Use |
|---|---:|---|
| `--ink-950` | `#111714` | Primary text and dark navigation |
| `--ink-700` | `#39443e` | Secondary text |
| `--ink-500` | `#66736c` | Muted metadata; verify contrast before small text |
| `--paper-50` | `#f7f6f1` | Application background |
| `--paper-100` | `#efeee7` | Subtle grouping and table hover |
| `--paper-0` | `#fffefa` | Primary surfaces |
| `--line-200` | `#d8d8cf` | Dividers and borders |
| `--teal-700` | `#156b63` | Primary action, selected navigation, focus companion |
| `--teal-100` | `#d8ebe7` | Calm selected/success support background |
| `--amber-700` | `#9a5b08` | Waiting, due, degraded, and estimated |
| `--amber-100` | `#f5e8c9` | Warning background |
| `--red-700` | `#a43d38` | Failed, destructive, protection gap |
| `--red-100` | `#f5dedb` | Critical background |
| `--blue-700` | `#315e8a` | Informational or active read operations |

Status is never communicated only by color. Every indicator includes a label and, where needed, an icon or shape.

### Typography

- UI sans: `Inter`, `Source Sans 3`, or the system sans stack. Use tabular numerals for capacity, throughput, and dates.
- Operational mono: `IBM Plex Mono`, `Source Code Pro`, or system monospace for tape IDs, barcodes, job IDs, paths, versions, and checksums.
- Base size: 15–16 px desktop, never below 14 px for operational data.
- Headings are compact and sentence case. Avoid oversized marketing typography.
- Use full timestamps with timezone in details; concise local dates may appear in lists with an accessible full value.

### Spacing and shape

Use a 4 px base scale: 4, 8, 12, 16, 24, 32. Tables generally use 10–12 px vertical cell padding. Surface radius is 4–6 px; pills are reserved for short statuses and filters. One-pixel dividers create structure. Shadows are rare and limited to menus/dialogs.

## Application shell

Desktop uses a persistent 224–248 px left navigation, a compact top context bar, and a fluid content canvas. Navigation order:

1. Overview
2. Jobs
3. Sources
4. Tapes & pools
5. Drives & libraries
6. Restore
7. Off-site rotations
8. Verification & recovery
9. Notifications
10. Settings

The top bar contains environment identity, simulator/production badge, global search, current alerts, and account menu. The simulator badge is persistent and cannot be dismissed.

Phone navigation uses a top title/action row plus a compact menu. Critical movement confirmation must not hide the exact tape ID, source/destination, or coverage consequence behind another screen.

## Reusable patterns

### Status label

Shows an icon/shape, explicit text, and optional timestamp. Vocabulary is controlled: Queued, Preparing, Waiting for media, Writing, Written, Verifying, Verified, Blocked, Interrupted, Failed, Canceled. Do not collapse Written and Verified into “Complete.”

### Coverage strip

A horizontal compact summary shows latest backup, latest verified local recovery point, latest complete verified off-site recovery point, and gaps. Each cell includes source-data time and evidence state, not only an icon.

### Operator action row

Contains urgency, exact object/tape ID, required action, reason, due/overdue time, dependency impact, assignee, and one clear primary action. Secondary inspection remains visible.

### Dense data table

Tables support search, sortable columns, saved filters, keyboard row activation, sticky headers, column visibility, and a contextual details drawer. Long IDs and paths use mono type with copy action and accessible overflow expansion. Bulk actions show exact selection count and are disabled when mixed state is unsafe.

### Evidence block

Use a bordered block for verification scope, drive/media, algorithm, start/end, bytes read, outcome, errors, and links to immutable evidence. Sample, full read-back, and restore test have distinct labels.

### Timeline

Jobs, custody, and audit use append-oriented timelines. Corrections point to the event they supersede without erasing it. Actor, timestamp, evidence, and source (operator/device/system) are visible.

### Destructive preview

Use a full-page or large modal workflow with affected recovery points, dependencies, media, keys, coverage impact, and immutable preview ID. Require reauthentication only after review. The final button names the exact action, such as “Erase tape TS-0042,” rather than “Confirm.”

### Empty/loading/error states

- Empty: explain what the collection represents and the safe next action.
- Loading: preserve table/layout shape and announce progress to assistive technology.
- Waiting: show the exact external prerequisite and elapsed time.
- Failure: state what failed, protection impact, what remains safe, correlation ID, and recovery action.
- Partial: list included and excluded scope; never present it as success.

## Task-specific views

### Operational overview

Top order is protection gaps and simulator status, active work, required operator actions, coverage strip, drive activity, then recent failures/verification. Use small sparklines only when they communicate throughput or capacity trend; always pair them with values.

### Tape inventory

Default columns: tape ID, barcode, generation/format, pool, actual/observed location, custody confirmation, content group, protected until/indefinite, holds, verification, health, and current reservation. Planned destination never replaces actual location. Unknown media is visually protected, not merely “unassigned.”

### Job details

Show state and required action first, then progress with logical/encoded bytes, source/drive throughput, waiting time, tapes, snapshot identities, resolved source coverage, dependencies, transitions, logs, and verification evidence. Estimates carry an “Estimated” label.

### Drives and libraries

Show stable drive identity, model, serial/WWN, firmware, generation/capabilities, OS path, loaded media, controller lease, activity, cleaning/health, and reconciliation status. Library slot views represent actual element types and observed state; no decorative warehouse map.

### Restore planning

Use a staged but revisitable flow: recovery point and representation → content/scope → prerequisite/tape chain → approved destination/conflicts → validation plan → review. Missing prerequisites stay visible. Native stream file extraction clearly shows complete-chain staging requirements.

### Off-site rotations

Provide packing checklist, return checklist, and phone confirmation. Each tape row shows exact ID/barcode, current confirmed location, destination, reason, group completeness, and scan/manual check. Departure/export-slot and confirmed arrival are separate actions. Show coverage before and after the proposed movement.

## Responsive behavior

At narrow widths, tables become prioritized stacked rows rather than losing identifiers. The first line contains status and exact tape/job/source ID; the second contains action and location; expansion reveals metadata. Primary confirmation remains reachable but follows the impact summary.

Phone custody confirmation supports scanning when available and manual selection always. It must display mismatch errors before accepting confirmation and must not infer arrival from a scan alone without the configured confirmation step.

## Accessibility

- Meet WCAG 2.2 AA contrast and interaction requirements.
- Every operation is keyboard accessible with a visible, high-contrast focus ring.
- Use landmarks, a skip link, logical heading order, proper table headers, and native controls.
- Announce asynchronous state and validation errors without stealing focus.
- Do not encode status only through hue, position, animation, or icon.
- Respect reduced-motion preferences; no essential timed animation.
- Dialogs trap focus correctly and restore it to the invoking control.
- Touch targets are at least 44 by 44 CSS pixels on phone workflows.

## Content standards

Use direct operational language:

- “Waiting for tape TS-0042” rather than “Pending resource.”
- “Written; read-back not started” rather than “Complete.”
- “Off-site move planned” versus “Arrived at Iron Mountain — confirmed by A. Operator at …”.
- “Estimated 1.8 TB / 4–6 hours” rather than presenting estimates as exact.
- “Cannot reuse: retained recovery point RP-… depends on this tape” rather than “Not eligible.”

Errors explain failure, protection impact, what remains safe, and the next action. Do not recommend shell workarounds for routine operation.

## Review checklist for AT-31

Review overview, tape inventory, job details, restore planning, and off-site rotation at representative desktop and phone sizes with realistic long names and large inventories. For each, exercise empty, loading, waiting-for-media, interrupted, failed, partial, and successful states.

Record:

- task completion and decision clarity;
- consistency with the tokens and patterns above;
- keyboard, focus, screen-reader naming, contrast, and responsive behavior;
- correct distinctions between planned/confirmed, written/verified, estimate/measurement, and simulated/qualified;
- product-owner feedback, changes, unresolved issues, date, build, and reviewer.

A generic component-library demo or decorative dashboard does not pass this review.
