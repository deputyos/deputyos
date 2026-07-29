# 17 — Desktop runtime and resident agent

The deputyOS desktop product runs multiple isolated, named deputyOS instances
on Linux, macOS, and Windows. The Tauri application and CLI are two clients of
the same runtime; neither owns a separate lifecycle implementation.

“As many deputies as the user desires” has two distinct limits. A desktop may
hold a very large object-backed inventory of logical deputy definitions, while
only a host-safe working set is materialised or running. The runtime may
oversubscribe memory, but it must observe host pressure, reclaim idle guest
memory, pause cold deputies, and reject starts that would make the host
unstable.

Deputies move through three storage/runtime tiers:

1. **Defined** — a compact record in an immutable inventory segment; no ports,
   disk overlay, VM process or central database row.
2. **Materialised** — a content-addressed base image plus a private
   copy-on-write overlay and runtime configuration on the desktop.
3. **Active** — a running or paused VM/distro with `deputyd` inside it.

Materialisation and activation are reversible cache operations. The manifest
and user data remain the source of truth. The object layout and commit rules
are specified in [the control-data boundary](18-data-boundary.md).

## Control planes

There are three cooperating control planes:

1. The object-backed inventory controls deputy definitions and desired state.
2. The host runtime controls VM or distro state, virtual CPUs, allocated
   memory, disks, and networking.
3. `deputyd` runs inside every active image and controls the deputyOS workloads,
   guest memory preparation, filesystem durability, and guest telemetry.

The host must ask the guest to prepare before it pauses, checkpoints, or
aggressively balloons an instance. A host pause alone stops virtual CPUs but
does not make dirty state durable or guarantee that useful RAM can be
reclaimed.

## Resident agent

`deputyd` is baked into `/usr/local/bin/deputyd` and runs as
`deputyd.service`. It listens on `/run/deputyos/deputyd.sock`, mode `0600`.
It does not listen on TCP.

Host backends reach the Unix-socket API through the platform's local guest
execution mechanism:

| Host | Guest execution path |
|---|---|
| Linux QEMU/KVM | QEMU Guest Agent over a per-instance Unix socket |
| macOS UTM | UTM's QEMU Guest Agent scripting bridge |
| Windows WSL2 | `wsl -d <instance> -- deputyd ...` |

The JSON protocol is currently version 2 and accepts version 1 lifecycle
requests for image/desktop rolling upgrades:

```json
{
  "protocol": 2,
  "id": "host-request-id",
  "actor": {
    "kind": "account",
    "id": "account-id",
    "source": "cloud_api"
  },
  "command": "health"
}
```

The typed operation families are:

- agent health and capability discovery;
- pause/resume, snapshot quiesce/complete, and memory reclaim;
- active-workload status/start/stop/restart/reconcile;
- validated memory-high/memory-max, CPU quota, and I/O weight on
  `deputyos-workloads.slice`;
- update-slot and reconciliation status, plus allow-listed signed-update and
  repair runs;
- tunnel/network and filesystem capacity status, plus tunnel restart;
- bounded active-workload logs and bounded resident-agent audit events.

The API cannot name a program, shell command, filesystem path, URL, port, or
systemd unit. Profiles resolve through a compiled allowlist and diagnostic
counts are capped. Mutating socket requests are appended to a rotating,
root-owned JSONL event log with their request and actor IDs.

The profile gateway services run in `deputyos-workloads.slice`. The management
agent deliberately remains outside that slice so it can thaw workloads after
the host resumes the instance.

## Remote tunnel access

The integrated outbound tunnel already forwards the authenticated wizard on
port 8088. The wizard provides a narrow HTTP bridge to the resident agent:

| Method | Path | Resident command |
|---|---|---|
| `GET` | `/api/v1/runtime` | `health` |
| `GET` | `/api/v1/runtime/capabilities` | `capabilities` |
| `POST` | `/api/v1/runtime/command` | any typed v2 `AgentCommand` |
| `POST` | `/api/v1/runtime/prepare-pause` | `prepare_pause` |
| `POST` | `/api/v1/runtime/resume` | `resume` |
| `POST` | `/api/v1/runtime/reclaim` | `reclaim` |

These routes use the same security boundary as remote management:

- first-boot/local access uses the single-use launch token and session cookie;
- remote access uses an RS256 AccountOwner JWT whose `sub` must match the
  device's registered account;
- session cookies are `HttpOnly` and `SameSite=Strict`;
- no generic execute route exists, and request bodies cannot name a program or
  supply an argument vector.

The generic command route deserializes directly into the closed
`AgentCommand` enum: unknown variants, arbitrary fields and type mismatches
are rejected before the socket call. The bridge connects to the root-owned
Unix socket locally. The tunnel never connects to a privileged TCP listener.

The cloud command queue exposes the protocol asynchronously at
`POST /api/v1/devices/:id/commands`. The API validates the same operation
schema, adds a device-scoped actor, expiry and optional idempotency key, and
provides capability, history and per-command status routes. The device poller
passes the command ID and actor into `deputyd`; expired commands are
acknowledged without execution.

The guest-facing bridge can quiesce or reclaim the guest remotely, but a full
host VM pause/balloon operation still requires the desktop host control plane.
The tunnel exposes the full guest-safe v2 contract. It does not claim that a
guest HTTP request has paused its own
hypervisor. A later remote host-lifecycle channel must target the desktop
runtime and coordinate this guest bridge before operating the hypervisor.

## Host backend behaviour

| Backend | Pause/resume | Memory behaviour | Multi-instance endpoint |
|---|---|---|---|
| Linux QEMU/KVM | QMP `stop` / `cont`, coordinated with `deputyd` | Per-instance virtio balloon within the configured min/max envelope | Distinct localhost port forwards |
| Windows WSL2 | Cooperative quiesce, reclaim, terminate, and resumable host marker | WSL dynamically manages its shared utility VM; no dishonest per-distro live target is exposed | Distinct `netsh portproxy` remaps |
| macOS UTM | Saved suspend/resume through the UTM scripting bridge | Boot-time envelope; UTM has no supported live balloon target | Distinct shared-VLAN guest IP from QEMU Guest Agent |

On macOS, local wizard links use the guest IP rather than pretending UTM can
remap every VM to a unique localhost port. The outbound authenticated tunnel
does not depend on that LAN route and remains the external access path.

## Required lifecycle sequence

Pause with retained VM memory:

```text
deputyd prepare_pause
host pause/suspend
```

Resume:

```text
host resume
deputyd resume
```

Pressure-driven memory reclaim:

```text
deputyd reclaim
host set balloon target
observe guest and host pressure
```

Hibernate/checkpoint, which releases substantially more host memory than a
pause, additionally saves VM state to disk and stops the VM process.

## Image verification

The QEMU smoke harness attaches a real virtio QEMU Guest Agent channel and
checks:

- `deputyd` is executable and its service is active;
- its socket is owner-only;
- health reports protocol version 2 and active lifecycle state;
- QEMU Guest Agent is active;
- prepare-pause and resume make a successful round trip.

These checks are image gates, not host-side mocks. Unit tests separately cover
protocol compatibility, telemetry parsing, idempotence, rollback after a
failed sync, memory reclaim, and Unix-socket request/response behaviour.
