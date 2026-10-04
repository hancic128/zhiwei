# Remove Service Layer — Design Spec

**Date:** 2026-10-04
**Status:** Approved

## Problem

The current design requires users to create a "service" first, then add probes under it. This is unnecessarily complex — the service layer is just a grouping mechanism with a name and description. The UI showed Chinese service names ("对外服务", "中文站") in English mode because service names are user-input strings stored directly in the database.

Users want to create probes directly without the service abstraction.

## Solution

Remove the service layer entirely. Probes become top-level entities with their own name and description.

## Database Changes

### Migration: mXXX_remove_service_layer

1. **Drop `services` table** — no migration needed, just `DROP TABLE services`
2. **Modify `probes` table:**
   - Add `description` column: `TEXT DEFAULT ''`
   - `service_id` column: **drop** (no foreign key needed)
   - `service_name` column: **rename to `name`** (already named correctly, just update references)

### Schema After Migration

```sql
CREATE TABLE probes (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT DEFAULT '',        -- NEW: was service description
    kind TEXT NOT NULL,
    target_json TEXT DEFAULT '{}',
    expect_json TEXT DEFAULT '{}',
    interval_seconds INTEGER DEFAULT 60,
    timeout_ms INTEGER DEFAULT 5000,
    failure_threshold INTEGER DEFAULT 3,
    node_ids_json TEXT DEFAULT '[]',
    location TEXT NOT NULL DEFAULT 'node',
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at_unix_nano INTEGER NOT NULL,
    updated_at_unix_nano INTEGER NOT NULL
    -- REMOVED: service_id, service_name
);
```

### Data Migration

**No data migration.** The existing `services` table is empty in production, so dropping it has no data loss impact.

## API Changes

### `POST /v1/probes` (create_probe_handler)

**Before:**
```rust
struct Body {
    service_id: String,  // REQUIRED
    name: String,
    kind: String,
    // ...
}
```

**After:**
```rust
struct Body {
    name: String,
    description: String,  // NEW
    kind: String,
    // ... (service_id removed)
}
```

**Changes:**
- Remove `service_id` validation and field
- Add `description` field (defaults to empty string)
- Update `ProbeInput` struct accordingly

### `PATCH /v1/probes/{id}`

Add `description` to updateable fields.

### `GET /v1/services` (list_services_handler)

**Remove entirely.** Return 404 for backward compatibility or redirect to `/v1/probes`.

### Other Handlers

- `create_service_handler` → remove
- `patch_service_handler` → remove
- `delete_service_handler` → remove
- `get_service_handler` → remove

## Backend Logic Changes

### Alert Messages (alerts.rs)

**Before:**
```rust
let rule_name = format!("Service {} · {}", probe.service_name, probe.name);
let message = format!("Probe {} for service {} is down: {detail}",
    probe.name, probe.service_name, detail=detail);
```

**After:**
```rust
let rule_name = probe.name.clone();
let message = format!("Probe {} is down: {detail}", probe.name, detail=detail);
```

### ProbeView Response

```rust
struct ProbeView {
    id: String,
    name: String,
    description: String,  // NEW
    kind: String,
    // ...
    // REMOVED: service_id, service_name
}
```

## Frontend Changes

### Page: Services → Probes

**Rename page from "Services" to "Probes"** in the sidebar and page title.

### Remove ServiceDialog

The `ServiceDialog` component is no longer needed. Remove:
- The dialog component
- `serviceDialog` state
- "New service" button

### Change "New Service" → "New Probe"

```tsx
// Before
<Button onClick={() => setServiceDialog({ open: true })}>
  <Plus className="w-4 h-4" />
  {t("services.newService")}
</Button>

// After
<Button onClick={() => setProbeDialog({ open: true })}>
  <Plus className="w-4 h-4" />
  {t("probes.newProbe")}
</Button>
```

### Add Description to ProbeDialog

Add a description input field to `ProbeDialog`:
```tsx
<Field label={t("probes.description")} hint={t("probes.descriptionHint")}>
  <Input
    value={description}
    onChange={(e) => setDescription(e.target.value)}
    placeholder={t("probes.descriptionPlaceholder")}
  />
</Field>
```

### Update List Display

**Before:**
```tsx
<div>
  <div className="font-medium">{p.name}</div>
  <div className="text-xs text-ink-400">
    {r.service_name}
    <DotBadge>{t("services.enabled")}</DotBadge>
  </div>
</div>
```

**After:**
```tsx
<div>
  <div className="font-medium">{p.name}</div>
  {p.description && (
    <div className="text-xs text-ink-400 truncate max-w-[220px]">
      {p.description}
    </div>
  )}
</div>
```

### Translation Keys

New keys needed:
```json
{
  "probes": {
    "title": "Probes",
    "subtitle": "Monitor your services",
    "newProbe": "New probe",
    "description": "Description",
    "descriptionHint": "Optional description for this probe",
    "descriptionPlaceholder": "e.g. Monitor blog.hancic.site availability",
    "empty": "No probes configured",
    "emptyHint": "Create a probe to start monitoring"
  }
}
```

Rename existing keys from `services.*` to `probes.*`:
- `services.colProbeName` → `probes.colProbeName`
- `services.colKind` → `probes.colKind`
- etc.

## Testing Checklist

- [ ] Create probe without service_id → should succeed
- [ ] Probe appears in list with description
- [ ] Alert message shows only probe name (not "Service X · Y")
- [ ] English/Chinese toggle shows correct translations
- [ ] `GET /v1/services` returns appropriate response (404 or redirect)
- [ ] Database migration runs without errors
- [ ] Existing probe data preserved (service_name → name)

## Files to Modify

1. `crates/storage/src/migrations.rs` — new migration
2. `crates/storage/src/probes_repo.rs` — schema, queries
3. `crates/monitor-server/src/probes_api.rs` — API handlers
4. `crates/monitor-server/src/routes.rs` — remove service routes
5. `crates/monitor-server/src/alerts.rs` — alert message format
6. `ui/src/pages/services.tsx` — rename to probes.tsx, UI changes
7. `ui/src/api.ts` — update API calls
8. `ui/locales/en-US/common.json` — translations
9. `ui/locales/zh-CN/common.json` — translations
10. `ui/src/navigation.tsx` — rename "Services" to "Probes"

## Rollback Plan

If issues arise:
1. Revert migration by restoring `services` table and `service_id` column
2. Add back service_id to ProbeInput
3. Restore API validation
4. Revert frontend changes
