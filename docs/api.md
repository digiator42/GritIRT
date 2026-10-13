# GritIRT API contract

Backend: GritShield incident-response service at `backend/`. Port **8030** by
default (`IRT_PORT`), bind `127.0.0.1` (`IRT_BIND`). All endpoints are JSON.
When `IRT_API_KEY` is set, every request must present the key via the
`X-API-Key` header or `?api_key=` query parameter (the query form exists because
`EventSource` cannot set headers).

GritIRT ingests alerts from the rest of the family (CyberFlow / `logs-soc`,
LuxShield, the ACC), **correlates** them into incidents, runs **approval-gated
playbooks**, keeps a **timeline/audit**, and exports **postmortems**.

---

## Data model

### Incident

```json
{
  "id": 12,
  "correlation_key": "luxshield:port_scan:203.0.113.9",
  "status": "open",
  "severity": "high",
  "title": "[LUXSHIELD] 203.0.113.9",
  "summary": "",
  "source": "luxshield",
  "entity": "203.0.113.9",
  "alert_count": 3,
  "first_seen_ms": 1720524000000.0,
  "last_seen_ms": 1720524060000.0,
  "assignee": "",
  "created_at": 1720524000123,
  "updated_at": 1720524060000
}
```

`status` ∈ `open | triaging | investigating | contained | eradicating |
recovering | resolved | closed`.
`severity` ∈ `low | medium | high | critical` (any other input normalises to
`medium`). `ts_ms`/`*_ms` fields are epoch milliseconds as floats; `created_at`
/ `updated_at` / `received_at` are integer epoch milliseconds.

### Alert

```json
{
  "id": 44,
  "incident_id": 12,
  "source": "luxshield",
  "kind": "port_scan",
  "severity": "critical",
  "entity": "203.0.113.9",
  "detail": { "attempts": 512 },
  "correlation_key": "luxshield:port_scan:203.0.113.9",
  "received_at": 1720524060000
}
```

### Timeline event

```json
{
  "id": 91,
  "incident_id": 12,
  "run_id": 5,
  "actor": "system",
  "kind": "step_succeeded",
  "payload": { "step": "block", "seq": 1, "output": { "simulated": true } },
  "ts_ms": 1720524060500.0
}
```

`kind` values: `incident_opened`, `alert_merged`, `status_changed`, `assigned`,
`note`, `run_started`, `step_started`, `step_succeeded`, `step_failed`,
`awaiting_approval`, `step_approved`, `step_rejected`, `run_finished`,
`run_failed`, `postmortem_created`.
`actor` ∈ `system | analyst | auto | mcp`.

---

## Correlation & dedup

Every alert gets a **correlation key**: the caller may pass one explicitly,
otherwise it is `source:kind:entity` (lower-cased). An incoming alert is merged
into the newest **non-resolved/non-closed** incident with the same key whose
`last_seen_ms` is within the dedup window (`IRT_DEDUP_WINDOW_MS`, default
5 min = `300000`); otherwise a new incident is opened. Merging bumps
`alert_count`, refreshes `last_seen_ms`, and escalates `severity` to the max of
the two.

---

## Endpoints

### `GET /health`

```json
{
  "service": "gritirt-backend",
  "status": "ok",
  "db": true,
  "feed": {
    "history_count": 0,
    "history_capacity": 2000,
    "live_streams": 0,
    "published_total": 0,
    "attached_total": 0
  },
  "connectors": ["luxshield", "cyberflow"]
}
```

200 = up. `connectors` lists the outbound connectors enabled by env
(`IRT_LUXSHIELD_URL`, `IRT_CYBERFLOW_URL`, `IRT_ACC_URL`).

### `POST /api/v1/alerts`

Ingest one alert, correlate it, and — **only when a new incident is opened** —
auto-start the first enabled playbook whose `auto_severity` threshold the
incident meets.

Request:

```json
{
  "source": "luxshield",
  "kind": "port_scan",
  "severity": "high",
  "entity": "203.0.113.9",
  "detail": { "attempts": 512 },
  "ts_ms": 1720524000000,
  "correlation_key": null
}
```

Required: `source`, `severity`. `kind` defaults to `generic`, `entity` to
`unknown`, `detail` to `{}`, `ts_ms` to server time, `correlation_key` to the
computed key.

Response `201` when a new incident was opened, `200` when merged:

```json
{
  "incident": { "...": "IncidentView" },
  "alert": { "...": "AlertView" },
  "created_incident": true,
  "auto_run_id": 7
}
```

`auto_run_id` is `null` when no playbook matched.

### `GET /api/v1/incidents?limit=&status=&severity=`

`limit` default 50, clamped 1..500.

```json
{ "count": 2, "incidents": [ { "...": "IncidentView" } ] }
```

### `GET /api/v1/incidents/:id`

Full dossier:

```json
{
  "incident": { "...": "IncidentView" },
  "alerts": [ { "...": "AlertView" } ],
  "timeline": [ { "...": "TimelineEventView" } ],
  "runs": [ { "...": "RunView" } ]
}
```

404 `{"error":"incident_not_found","id":12}` when missing.

### `POST /api/v1/incidents/:id/status`

Body `{ "status": "investigating" }`. Records a `status_changed` timeline event
and emits an `incident` SSE frame. Validation: unknown values normalise to
`open`.

```json
{ "incident": { "...": "IncidentView" } }
```

### `POST /api/v1/incidents/:id/assign`

Body `{ "assignee": "alice" }`. Timeline `assigned`; emits `incident`.

### `POST /api/v1/incidents/:id/notes`

Body `{ "text": "contained at edge", "actor": "alice" }` (`actor` optional,
default `analyst`). Returns `201 {"event_id": 91}` and emits a `timeline` frame.

### `GET /api/v1/incidents/:id/timeline`

```json
{ "count": 6, "timeline": [ { "...": "TimelineEventView" } ] }
```

Ascending by id (chronological).

### `GET /api/v1/incidents/:id/alerts`

```json
{ "count": 3, "alerts": [ { "...": "AlertView" } ] }
```

Newest first.

### `POST /api/v1/incidents/:id/playbook`

Start a runbook against an incident.

```json
{ "playbook_id": 3 }
```

`playbook_id` optional — when omitted, GritIRT picks the first enabled playbook
whose `auto_severity` threshold the incident meets. `201`:

```json
{ "run_id": 7, "playbook_id": 3 }
```

400 `{"error":"no_playbook_matched"}` when none matches; 404 for a missing
incident or playbook.

### `GET /api/v1/incidents/:id/runs`

```json
{ "count": 2, "runs": [ { "...": "RunView" } ] }
```

### `GET /api/v1/runs/:id`

Run header + its steps:

```json
{ "run": { "...": "RunView" }, "steps": [ { "...": "RunStepView" } ] }
```

`RunView`: `{ id, playbook_id, incident_id, status, initiator, started_at,
finished_at }`. `status` ∈ `pending | running | waiting_approval | succeeded |
failed | cancelled`.

`RunStepView`: `{ id, run_id, seq, name, status, gate, gate_status, output,
started_at, finished_at }`. `status` ∈ `pending | running | succeeded | failed |
cancelled`; `gate_status` ∈ `none | open | approved | rejected`.

### `POST /api/v1/runs/:id/approve`

Decide a gated step; the run then resumes (approve) or stops (reject).

```json
{ "seq": 1, "approve": true }
```

`approve` defaults to `true`. `seq` is the zero-based step index within the run.
`200 → {"run_id":7,"seq":1,"approved":true}`; 404 `run_not_found` /
`step_not_found`.

### `GET /api/v1/playbooks`

```json
{ "count": 1, "playbooks": [ { "...": "PlaybookView" } ] }
```

### `POST /api/v1/playbooks`

Create a playbook (runbook). `auto_severity` empty = manual-only.

```json
{
  "name": "Contain hostile IP",
  "description": "Note, then blackhole the entity",
  "auto_severity": "high",
  "steps": [
    { "name": "note",
      "action": { "kind": "sim", "text": "triaging" } },
    { "name": "block", "gate": true,
      "action": { "kind": "mcp", "connector": "luxshield",
                  "tool": "add_firewall_rule",
                  "arguments": { "ip": "{{incident.entity}}", "action": "drop" } } },
    { "name": "file",
      "action": { "kind": "http", "method": "POST",
                  "url": "http://127.0.0.1:8040/rest/api/2/issue",
                  "headers": { "Content-Type": "application/json" },
                  "body": { "fields": { "summary": "{{incident.title}}" } } } }
  ]
}
```

`201 → {"playbook_id":3,"name":"Contain hostile IP"}`. 400 `name_required` /
`steps_required` / `invalid_playbook`.

**Step action kinds:**

- `sim` — `{ "text": "..." }`. No external call.
- `mcp` — call an MCP tool on a named connector:
  `{ "connector": "luxshield", "tool": "add_firewall_rule", "arguments": { ... } }`.
  The connector must be configured via env. GritIRT POSTs a JSON-RPC
  `tools/call` to `{base_url}/mcp`.
- `http` — generic JSON HTTP call:
  `{ "url": "...", "method": "POST", "headers": { ... }, "body": { ... } }`
  (`method` defaults to `POST`).

**Templating:** any string in `arguments`/`body`/`url` may reference
`{{incident.id}}`, `{{incident.entity}}`, `{{incident.title}}`,
`{{incident.severity}}`, `{{incident.status}}`, `{{incident.source}}`, and
`{{now_ms}}`. Substitution happens at execution time.

**Gates:** a step with `"gate": true` pauses the run in `waiting_approval`
before it executes. Resolve it with `POST /api/v1/runs/:id/approve` (or the
`approve_step` MCP tool). Approve → step runs; reject → run goes `cancelled`.

### `GET /api/v1/playbooks/:id`

```json
{ "playbook": { "...": "PlaybookView" } }
```

### `POST /api/v1/incidents/:id/postmortem`

Generate and store a Markdown postmortem from the incident's alerts + timeline.
`201`:

```json
{ "postmortem_id": 1, "content": "# Postmortem: ...\n..." }
```

Also records a `postmortem_created` timeline event.

### `GET /api/v1/incidents/:id/postmortem`

Most recently generated document:

```json
{ "id": 1, "incident_id": 12, "content": "# Postmortem: ...", "created_at": 1720524061000 }
```

404 `postmortem_not_found` when none exists yet.

### `GET /api/v1/stream`

Server-Sent Events feed. Replays the newest `history` frames (default 50, max
500) then live frames. **Event names:** `incident`, `alert`, `timeline`, `run`,
`step`. Each `data:` line is the JSON object shown above for that type.

```text
event: incident
data: {"id":12,"status":"open","severity":"high", ...}

event: step
data: {"run_id":7,"incident_id":12,"seq":1,"name":"block","status":"succeeded", ...}
```

`EventSource` reconnects automatically; ignore keep-alive comment frames. Replay
history is **newest-first** — reverse before charting.

---

## MCP (`POST /mcp`, JSON-RPC 2.0)

Mounted automatically by GritShield at `/mcp` (relocate with `MCP_HTTP_PREFIX`).

### tools/list

`ingest_alert`, `list_incidents`, `get_incident`, `set_incident_status`,
`run_playbook`, `approve_step`, `export_postmortem`.

### Example — `tools/call ingest_alert`

```json
{"jsonrpc":"2.0","id":1,"method":"tools/call",
 "params":{"name":"ingest_alert",
           "arguments":{"source":"cyberflow","severity":"critical",
                        "kind":"brute_force","entity":"198.51.100.7"}}}
```

### Example — `tools/call approve_step`

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call",
 "params":{"name":"approve_step","arguments":{"run_id":7,"seq":1,"approve":true}}}
```

### prompts/get triage_incident

Argument `incident_id`. Returns an analyst-facing triage brief built from the
incident's alerts and response timeline.

---

## Error semantics

- `400 invalid_json` / `invalid_alert` / `invalid_playbook` / `invalid_id` —
  malformed request.
- `400 source_required` / `status_required` / `assignee_required` /
  `text_required` / `seq_required` — missing field.
- `401 unauthorized` — missing/bad API key (`IRT_API_KEY` set).
- `404 incident_not_found` / `run_not_found` / `step_not_found` /
  `playbook_not_found` / `postmortem_not_found`.
- `413 payload_too_large` — body over 1 MiB (framework also caps the whole
  request at 1 MiB).

---

## Configuration

| Env | Default | Meaning |
| --- | --- | --- |
| `IRT_BIND` | `127.0.0.1` | Bind address. |
| `IRT_PORT` | `8030` | Listen port. |
| `IRT_DATABASE_URL` | `sqlite://grit_irt.db?mode=rwc` | SQLite (file) URL. |
| `IRT_API_KEY` | unset (open) | Require `X-API-Key` / `?api_key=`. |
| `IRT_DEDUP_WINDOW_MS` | `300000` | Correlation window in ms. |
| `IRT_LUXSHIELD_URL` | unset | Enables `luxshield` connector. |
| `IRT_CYBERFLOW_URL` | unset | Enables `cyberflow` connector. |
| `IRT_ACC_URL` | unset | Enables `acc` connector. |
| `IRT_GRITSCAN_URL` | unset | Enables `gritscan` connector. |
| `IRT_<NAME>_API_KEY` | unset | Forwarded as `X-API-Key` on that connector's MCP calls. |

---

## Notes for the frontend

- **Base URL.** Call the backend directly, e.g.
  `BASE_URL = "http://127.0.0.1:8030"`. There is no nginx API proxy; relative
  paths 404 unless you add one. Keep `with_base(path)` (empty base → path
  unchanged) so a same-origin deploy only needs `BASE_URL = ""`.
- **API key.** Because `EventSource` cannot send headers, pass
  `?api_key=...` on the stream URL when the backend is locked down.
- **Live updates.** Prefer the SSE stream (`/api/v1/stream`) over polling. Event
  names map 1:1 to object types: `incident` (create/update), `alert` (new alert
  correlated), `timeline` (new timeline entry), `run` (run status change),
  `step` (step finished). Re-fetch `GET /api/v1/incidents/:id` on an `incident`
  or `timeline` frame if you want the fully hydrated dossier.
- **Severity is monotonic within a merge.** A merged incident escalates to the
  max of its alerts' severities; render the badge from `incident.severity`.
- **Approval UX.** A run in `waiting_approval` is blocked on the step whose
  `gate_status == "open"`. Show that step with Approve/Reject buttons that POST
  `/api/v1/runs/:id/approve` with `{seq, approve}`. After the decision the run
  moves to `running` then a terminal state; watch for the `run`/`step` frames.
- **Timeline is append-only.** Use `GET /api/v1/incidents/:id/timeline`
  (ascending) for the audit view; treat it as immutable history.
- **Postmortems are Markdown.** `GET /incidents/:id/postmortem` returns the
  latest `content`; render it as Markdown. `POST` regenerates from current data.
- **Empty base vs. offline services.** `connectors` in `/health` tells you which
  integrations are wired; a playbook `mcp` step fails (and fails the run) if its
  connector is unreachable — surface `run.status == "failed"` plus the failing
  step's `output.error`.
