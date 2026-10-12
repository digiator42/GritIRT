# GritIRT

Incident-response orchestration backend built on
[GritShield](https://crates.io/crates/gritshield). GritIRT turns raw alerts from
the rest of the family (CyberFlow / logs-SOC, LuxShield, the Admin Command
Center) into correlated **incidents**, drives **approval-gated playbooks** that
contain the threat across the fleet, keeps an append-only **timeline**, and
exports **postmortems**.

This repository contains the **backend only**. The operator console is a Velo
frontend deployed separately; its contract lives in [`docs/api.md`](docs/api.md).

## What it does

- **Ingest** — `POST /api/v1/alerts` accepts alerts from any probe.
- **Correlate** — alerts are keyed `source:kind:entity` (or an explicit key) and
  merged into an open incident within a dedup window; severity escalates to the
  max observed.
- **Auto-trigger** — opening an incident auto-starts the first enabled playbook
  whose `auto_severity` threshold the incident meets.
- **Runbooks** — playbooks are ordered steps; each can call an MCP tool
  (`luxshield`, `cyberflow`, `acc`), make a generic HTTP call (e.g. GritJira),
  or simulate. Steps may be **gated** behind analyst approval.
- **Audit** — every state change is written to the incident timeline and fanned
  out over SSE.
- **Postmortem** — one call renders a Markdown postmortem from the alert and
  response history.
- **MCP surface** — the same workflows are exposed as MCP tools for agents.

## Layout

```
backend/          Rust crate (GritShield + SeaORM/SQLite)
docs/api.md       Frontend/API contract
```

## Run

```bash
cd backend
cargo run --release
```

Listens on `http://127.0.0.1:8030` by default. The MCP surface is mounted at
`/mcp`.

### Configuration

| Env | Default | Meaning |
| --- | --- | --- |
| `IRT_BIND` | `127.0.0.1` | Bind address. |
| `IRT_PORT` | `8030` | Listen port. |
| `IRT_DATABASE_URL` | `sqlite://grit_irt.db?mode=rwc` | SQLite URL. |
| `IRT_API_KEY` | unset (open) | Require `X-API-Key` / `?api_key=`. |
| `IRT_DEDUP_WINDOW_MS` | `300000` | Correlation window (ms). |
| `IRT_LUXSHIELD_URL` | unset | Enables the `luxshield` connector. |
| `IRT_CYBERFLOW_URL` | unset | Enables the `cyberflow` connector. |
| `IRT_ACC_URL` | unset | Enables the `acc` connector. |

## Quickstart

Create a runbook:

```bash
curl -s http://127.0.0.1:8030/api/v1/playbooks -H 'Content-Type: application/json' -d '{
  "name": "Contain hostile IP",
  "auto_severity": "high",
  "steps": [
    { "name": "note", "action": { "kind": "sim", "text": "triaging" } },
    { "name": "block", "gate": true, "action": {
        "kind": "mcp", "connector": "luxshield", "tool": "add_firewall_rule",
        "arguments": { "ip": "{{incident.entity}}", "action": "drop" } } }
  ]
}'
```

Report an alert (opens + auto-runs an incident):

```bash
curl -s http://127.0.0.1:8030/api/v1/alerts -H 'Content-Type: application/json' -d '{
  "source": "luxshield", "kind": "port_scan", "severity": "high",
  "entity": "203.0.113.9", "detail": { "attempts": 512 }
}'
```

Approve the gated step when the run reports `waiting_approval`:

```bash
curl -s http://127.0.0.1:8030/api/v1/runs/1/approve -H 'Content-Type: application/json' \
  -d '{ "seq": 1, "approve": true }'
```

See [`docs/api.md`](docs/api.md) for the full contract (endpoints, SSE event
names, MCP tools, error semantics).
