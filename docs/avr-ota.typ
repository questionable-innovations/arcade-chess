#set heading(numbering: "1.1")

= ATmega firmware update architecture

#block(
  fill: rgb("eef9ef"),
  stroke: 0.8pt + rgb("4f8f58"),
  inset: 9pt,
  radius: 4pt,
)[
  *Current implementation boundary.* Framed preflight, maintenance lease,
  two-phase token validation, alternating CRC-protected EEPROM markers,
  ACK-and-drain, LED shutdown, watchdog reset, ESP bus handoff, the ESP page
  programmer with full page readback verification, Intel HEX validation, and the
  health/confirm exchange are implemented. Bootloader-side marker handling, the
  signed artifact manifest, and the remote job lifecycle remain.
]

#block(
  fill: rgb("eef5ff"),
  stroke: 0.8pt + rgb("6a8bb8"),
  inset: 10pt,
  radius: 4pt,
)[
  *Requirement.* The ESP32 must update any one ATmega328PB through only the shared
  UART lines. There is no ESP-controlled reset pin. A user must be able to start
  the same operation remotely from the website and observe it through completion
  or recovery.
]

This is a required product capability, not a late manufacturing convenience. A
protected resident bootloader is installed during the initial ISP flash together
with fuses, lock bits, quadrant identity, and the first application image. Later
updates use a normal addressed application command, a software reset into the
bootloader, and a temporary point-to-point programming session controlled by the
ESP32.

== Proven reference and changes for Arcade Chess

The sibling `ec209-2025-project-2025_team_20` design demonstrates the essential
path successfully on an ATmega328PB and ESP32:

- the AVR application recognizes a UART command and transfers to a bootloader;
- the ESP switches the UART to programming mode;
- an STK500-compatible client writes 128-byte flash blocks;
- a browser uploads Intel HEX to the ESP; and
- programmer log lines are mirrored to the browser.

Arcade Chess retains the resident-bootloader and ESP programmer approach, but does
not copy several prototype shortcuts:

- A single byte such as `F` is unsafe on a multi-drop production protocol. Entry is
  an addressed, CRC-protected, two-phase command with a one-time token.
- Jumping from the UART ISR with a hard-coded address leaves interrupt, stack, and
  peripheral state ambiguous. The application records an update request and uses
  a watchdog software reset; the reset path and linker own the bootloader address.
- The reference HEX parser extracts line data without fully honoring record
  addresses, record types, checksums, gaps, or boot-section bounds. Arcade Chess
  normalizes and validates the artifact before it reaches programming mode.
- Reference readback verification is disabled. Arcade Chess cannot report success
  until readback verification and an application health handshake pass.
- Human log text is useful but is not machine state. Progress and errors are typed,
  sequenced events that survive browser or backend reconnects.

== Boot and flash memory contract

The exact bootloader size and start address are build outputs, not application
literals. The bootloader build must provide:

- a reserved boot section large enough for UART programming, update-marker logic,
  page readback, and application validation;
- matching `BOOTRST` and `BOOTSZ` fuse values;
- lock bits that allow application self-programming as intended but prevent the
  application image from overwriting the bootloader;
- an application linker limit below the boot section;
- the ATmega328PB signature, 128-byte flash page size, expected clock, UART rate,
  and protocol version in a machine-readable build manifest; and
- a reproducible bootloader binary and manufacturing command.

The first ISP operation always remains the recovery root. It writes and verifies
the bootloader, fuses, lock bits, quadrant ID, calibration defaults, and application.

There is not enough flash for a comfortable dual-bank application, so this design
provides *retry recovery*, not rollback. The bootloader is protected and remains
available after an interrupted application write. ISP is required only if the
bootloader/fuses themselves are damaged or the electrical bus cannot communicate.

== Persistent update marker

Before resetting, the application stores a small EEPROM record with CRC:

#table(
  columns: (1.2fr, 2.8fr),
  table.header([*Field*], [*Purpose*]),
  [Magic and schema], [Distinguish a valid marker from erased or corrupt EEPROM.],
  [Update ID], [Correlate AVR, ESP, backend, and browser state.],
  [Artifact identity], [Truncated SHA-256 or build ID expected by this session.],
  [Expected size/CRC], [Allow the bootloader to reject an incomplete application.],
  [State], [`requested`, `programming`, `candidate`, or `valid`.],
  [Trial boots], [Bound repeated unhealthy candidate starts before recovery mode.],
  [Record CRC], [Detect torn EEPROM writes.],
)

On any reset, the bootloader runs first. If the marker is requested/programming,
or the application validity check fails, it remains recoverable in update mode
instead of jumping into a partial image. A normal valid application receives only
a short boot window before execution.

EEPROM writes use an atomic two-slot or generation scheme so loss of power cannot
turn a partial record into a valid marker. Wear is insignificant for genuine
firmware updates but the command is rate-limited to avoid abusive writes.

== Addressed entry sequence

The normal framed protocol gains these messages:

#table(
  columns: (1.2fr, 2.8fr),
  table.header([*Message*], [*Meaning*]),
  [`FW_PREFLIGHT`], [Ask a target for identity, bootloader version/capabilities,
    app version, flash limits, reset cause, supply/health status, and update policy.],
  [`MAINTENANCE_BEGIN`], [Broadcast a no-response bus-quiet lease naming the target,
    update ID, start time, and maximum duration.],
  [`FW_PREPARE`], [Addressed image metadata and one-time entry token. The target
    validates compatibility and persists the update marker.],
  [`FW_ENTER_BOOTLOADER`], [Commit the prepared token. The target ACKs, drains TX,
    makes outputs safe, and causes a watchdog reset.],
  [`MAINTENANCE_END`], [Broadcast the result and return all non-target nodes to
    normal framed traffic. Expiry also restores them if the ESP resets.],
  [`FW_HEALTH`], [Application boot confirmation containing new build identity,
    self-test result, reset cause, and matching update ID.],
  [`FW_CONFIRM`], [ESP confirms the healthy candidate. The application atomically
    marks itself valid and ACKs; only this permits job success.],
)

The target never enters on malformed, broadcast, duplicated, stale, or wrong-ID
commands. Bootloader entry cannot be requested while a relay output is armed, the
target has an incompatible hardware revision, the artifact is absent from ESP
storage, or another maintenance job is active.

Non-target nodes continue sensing and local animation during the update but suppress
responses and ignore the raw programmer byte stream until the maintenance lease
ends. Their event queues must be large enough for the maintenance interval or must
set an overflow flag that triggers a later snapshot.

== Bootloader programming session

After the target ACK and watchdog reset, the ESP exclusively owns the UART and
changes from framed Arcade Chess traffic to the bootloader protocol. The initial
implementation may use the proven STK500v1-style command subset, provided its exact
behavior is captured in tests.

1. Synchronize and read bootloader/device identity.
2. Confirm target signature, flash geometry, boot bounds, and update ID.
3. Mark programming active.
4. Erase/write the normalized application image in 128-byte pages.
5. Retry a page only within a fixed policy and record every retry/error.
6. Read every programmed page back and calculate the expected image hash or CRC.
7. Reject any mismatch; do not clear the recovery marker.
8. Mark the candidate, leave programming mode, and software-reset the target.
9. Restore framed UART settings and wait for `FW_HEALTH` from the new application.
10. Send `FW_CONFIRM` for the matching update/build, wait for its ACK, and only then
    mark the job successful. The application atomically advances the marker to valid.

The validated programming baud must respect the diode-OR return rise time. The
implementation currently runs the programming session at 115,200 baud pending that
rise-time measurement, with 38,400 baud as the fallback rate. Normal quadrant
polling is suspended for the entire raw session.

If power, ESP, backend, or Wi-Fi disappears after programming starts, the update
marker keeps the target in a recoverable bootloader state. On ESP restart, it reads
its persisted job checkpoint, reacquires the target, and safely rewrites the whole
image unless a proven page-resume protocol is later added.

== Firmware artifact

The user may select Intel HEX, but the update system immediately converts it to a
canonical artifact. Validation must honor Intel HEX checksums, record addresses,
extended address records, gaps, EOF, duplicate/overlapping data, application bounds,
and boot-section exclusion.

The stored artifact contains a contiguous binary plus a signed or otherwise
authenticated manifest:

- target MCU and module/quadrant kind;
- compatible hardware revisions and quadrant application role;
- application version, build ID, source revision, and build time;
- bootloader and normal-protocol compatibility ranges;
- application start, byte length, padded length, and page count;
- SHA-256 of canonical bytes and optional application CRC used at boot;
- signing key ID and signature when release signing is introduced; and
- release notes and whether downgrade is permitted.

Both backend and ESP validate the artifact. The bootloader independently enforces
flash bounds and device identity even though the ESP is trusted to authenticate the
release.

#pagebreak()

== Remote website workflow

The browser does not keep the programming operation alive. Its workflow is:

1. Upload a firmware artifact to the Rust backend.
2. Backend parses, validates, hashes, stores, and reports manifest/preflight data.
3. An authorized operator selects one board and one or more compatible targets.
4. Backend creates a durable update job and asks the ESP to download the artifact
   over authenticated HTTPS.
5. ESP validates and stages the full artifact locally before disturbing the UART.
6. ESP updates targets one at a time. A four-quadrant update is four child jobs,
   never a broadcast flash.
7. Backend records structured progress events and fans them to every viewer.
8. A reconnecting browser loads the current job snapshot, then resumes the event
   stream from its last sequence.

The ESP continues an already staged flash if the website or backend disconnects.
ESP self-update and AVR update are mutually exclusive maintenance operations.

The UI separates upload/download/programming progress and shows:

- board, target address/identity, hardware revision, old and proposed versions;
- artifact hash, signature status, compatibility result, and release notes;
- current phase, current/total pages and bytes, elapsed time, retry count, and
  estimated progress without pretending an estimate is completion;
- latest structured error, whether it is retryable, and the exact recovery action;
- bus/node health before and after the job; and
- a timestamped diagnostic log export for support.

The upload HTTP response means only "artifact accepted". Success is displayed only
after page verification and the new application's health handshake. Cancellation
is allowed before destructive programming. After erase/write begins, the safe
choices are finish, retry, or explicitly leave the target recoverable in its
bootloader; the UI must not offer a misleading ordinary cancel.

== Observable update state machine

Every update has a backend-generated 128-bit `update_id`, and every target attempt
has an `attempt_id`. State is explicit:

```text
received -> validating -> waiting_for_device -> downloading -> staged
         -> preflight -> quieting_bus -> requesting_bootloader
         -> bootloader_sync -> programming -> verifying -> rebooting
         -> health_check -> succeeded

Any phase -> failed_retryable -> retrying
Any phase -> failed_recoverable_bootloader
Pre-destructive phases -> cancelled
Hardware/root failure -> failed_needs_isp
```

State transitions are monotonic and idempotent. ESP persists update ID, target,
artifact hash, phase, attempt, and last verified checkpoint in NVS. Backend stores
the job snapshot and append-only event history. The website never derives truth by
regex-parsing log sentences.

Critical transitions are delivered at least once and deduplicated by update ID,
ESP session, and event sequence. The backend acknowledges a sequence watermark.
The ESP does not write NVS for every page: it persists phase changes and bounded
page checkpoints, keeps rate-limited progress in memory, and publishes an
authoritative snapshot with cumulative counters after reconnect.

Each structured event includes at least:

#table(
  columns: (1.3fr, 2.7fr),
  table.header([*Group*], [*Fields*]),
  [Identity], [`update_id`, `attempt_id`, board ID, target address/UID, artifact
    build ID and SHA-256.],
  [Ordering], [ESP boot/session ID, event sequence, monotonic time, wall time when
    available.],
  [State], [Phase, prior phase, outcome, retryability, recovery classification.],
  [Progress], [Bytes/pages complete and total, current page/address, attempt and
    retry counts, phase start/elapsed time.],
  [Protocol], [Bootloader version, baud, last command, response/error code, timeout,
    UART framing/CRC counters.],
  [Verification], [Expected and observed hash/CRC, mismatch address/page, new app
    build ID and health result.],
)

Page progress may be rate-limited, for example every four pages or 250 ms, but
phase changes, retries, errors, and final verification are never sampled away. The
ESP retains a bounded raw UART trace around a failure and exposes cumulative update
metrics: success/failure by phase, duration, sync latency, page retries, readback
mismatches, interrupted recoveries, and ISP-required outcomes.

Human logs remain useful for engineers. They carry the same update/attempt IDs,
severity, component, and error code as structured events, are bounded/redacted, and
report dropped-log counts. Logs are supporting evidence, not control flow.

Error codes are stable enums grouped by source: `artifact_*`, `compatibility_*`,
`maintenance_*`, `boot_entry_*`, `boot_sync_*`, `flash_write_*`, `flash_read_*`,
`verify_*`, `candidate_health_*`, `confirm_*`, and `recovery_*`. An event carries
the stable code, retryability, recovery class, numeric protocol detail, and a human
message; dashboards aggregate by the code, never by the message.

== Failure behavior

#table(
  columns: (1.3fr, 2.7fr),
  table.header([*Failure*], [*Required behavior*]),
  [Artifact rejected], [No bus maintenance; report the exact manifest, checksum,
    signature, bounds, or compatibility error.],
  [Target will not enter], [End maintenance lease, restore normal nodes, preserve
    old application, report boot-entry handshake evidence.],
  [Sync fails], [Retry within policy; target remains in bootloader via marker.],
  [Page write/read mismatch], [Retry page, then whole image; never report success.],
  [Power/ESP reset], [Target remains bootloader-recoverable; ESP resumes job from
    persisted metadata and safely rewrites.],
  [Backend/browser loss], [ESP completes locally; state reconciles on reconnect.],
  [New app unhealthy], [Keep job failed-recoverable and re-enter/retry bootloader;
    do not clear the pending marker.],
  [Bootloader inaccessible], [Stop automated retries and mark `failed_needs_isp`
    with manufacturing/service instructions.],
)

#pagebreak()

== Acceptance tests

- Verify fuse/lock/linker boundaries and bootloader protection from application
  writes.
- Reject wrong MCU, hardware revision, bootloader range, corrupt HEX checksum,
  overlap, missing EOF, out-of-range address, boot-section data, and wrong hash.
- Prove an accidental byte sequence, malformed frame, replay, broadcast, duplicate,
  or wrong target cannot enter the bootloader.
- Interrupt power at every flash page and every state transition; prove the target
  returns to a programmable state without ISP.
- Reset ESP during entry, write, verify, reboot, and health-check phases.
- Disconnect backend and browser independently; prove local completion and later
  state reconciliation.
- Inject UART corruption, timeout, duplicate response, and slow rising edges while
  retaining actionable traces and counters.
- Update each quadrant alone and all four sequentially while non-target nodes remain
  electrically quiet, keep local effects smooth, and recover their state snapshots.
- Corrupt a programmed page and prove readback prevents success.
- Run at least 100 repeated update cycles per target/hardware revision and track
  duration, retries, and failure rates.
