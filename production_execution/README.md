# Production Execution

`ProductionExecution` owns production orders, completed operation executions,
material hold overrides and shop-floor capture. Resolve `SERVICE`
(`factory/production_execution.production_execution`) from the service
registry. Orders, capture and the shop-floor/configuration pages are implemented. It mirrors
Bilimbi `ProductionExecution` at `f39ed3dcf2ce2694d30c2b88e82212780f4f3b98`.

## Facade

Order and execution reads take a sealed scope and company, require
`factory.production-execution.floor.view`, and check the live company. Every
order/completion write rechecks `factory.production-execution.configuration.manage` and the
authorized writable company inside `MutationBoundary`. `tenant_owner` holds
`floor.view`, `configuration.view` and `configuration.manage`; an administrator
grants `import` and `material-hold.override` explicitly, as in Bilimbi.
Bilimbi's order and execution reads had no capability (a live company only);
Bilibi's facade-only writes and reads need the capabilities above, which a
floor operator who completes operations also needs alongside Product
Definition's and Inventory's view and manage keys (a completion reads
Product Definition and posts through Inventory).

- `create_order(NewOrder)`: `code` (company-unique, at most 255 characters),
  `kind` (`Order` or `Batch`), `product_id`, exact `formula_version` and
  `routing_version`, and an optional opaque `demand_ref`. The revisions are
  validated through `ProductDefinition::select_revisions` and retained on the
  order and on every execution.
- `get_order`, `list_orders` (newest first, 50 by default, clamped to 1-500 as
  Bilimbi does), `list_executions` (most recently completed first; 100 by
  default, at most 500, following the bounded-read convention), `get_execution`.
- `complete_operation(order, Source, Completion)`: one routed operation, live
  or imported. Both validate and post identically; an import first needs
  `factory.production-execution.import` (`Import not authorized` before
  anything is posted, and again on every retry). The `Completion` carries a
  company-unique `request_id` (at most 240 bytes), `operation_code`,
  `resource_id` (one the operation allows), `operator_type`/`operator_id` (who
  did the work), `started_at`/`completed_at` (completion not in the future),
  `evidence`, the actual `inputs` and `outputs` as Inventory `StockLine`s,
  optional Inventory `Variance` evidence (only with both inputs and outputs)
  and an optional `hold_override`. Lines must name an item of the selected
  operation, a location and a positive quantity; lines may mix native units and
  Inventory balances each unit on its own. An identical retry of a
  `request_id` returns the original execution and posts nothing; the same ID
  with different content is `Request id conflict`.
- `list_hold_overrides(execution)`: the immutable override evidence.

## Atomic posting

The execution row, Inventory's material effects and any override evidence
commit in **one** `MutationBoundary` transaction, or none does. The module
contributes a private `static PostingAuthority` to
`factory/inventory.posting_authorities` (`completion.rs`) and posts through
Inventory's `_on` production functions, which run on the caller's open
transaction (`record_transform_on` for inputs and outputs, `record_output_on`
for outputs only, `record_production_consumption_on` for inputs only). Inventory
still applies its own capability, company and idempotency checks; its
transaction `request_id` is `pe:<request_id>` and carries the run's context
(`operation_execution` = the request ID, `order_or_batch` = the order code,
`work_centre` = the resource ID). The company row lock serializes concurrent
retries.

## Material holds

A Formula input's `material_hold_rule` (`{"hours": n}`), or a Formula or
routing `process_config.material_hold_rules` entry keyed by the input's item ID
(an object or a list of objects), makes each matching actual input name an
Inventory `identity_id`; the longest minimum governs, and a rule without a
positive integer `hours` is `Invalid material hold rule`. The hold runs from
the identity's source transaction (a receipt, output or transform that put the
identity into stock, not after the completion) to the completion time; a missing
identity, a mismatched item or a future source is `Invalid hold source`.
Consuming early without a `hold_override` is `Material held`.

A `HoldOverrideRequest` is authorized as the Scope's own user in the order's
company with `factory.production-execution.material-hold.override` (otherwise
`Hold override denied`); an impersonated session is refused
(`Override refused under impersonation`). The decision runs before the posting
transaction, so a refused attempt keeps its decision log. Callers cannot name
the recorder. A live override carries only a `reason`; the user is both
approver and recorder, and an agent is recorded as the agent acting for the
user. An import also supplies `evidence`, the historical `occurred_at` (not
after completion) and optionally the historical `approver`; that approver is
source evidence only, and the Scope's user is recorded as `recorded_by_*`. An
already recorded request is not checked again.

## Data

Nine tables with tenant and company ownership; product, resource and Inventory
references are opaque IDs the sibling facades validated (no SQL dependence on
their tables, no cross-container reference). PostgreSQL refuses UPDATE, DELETE
and TRUNCATE on executions and overrides. Every saved row is audited with
complete old/new data. There are no production seeds, domain CSS or JavaScript.
Reviewed agent operations are described below.

## Shop-floor capture (XP2)

Configuration uses `create_`, `update_`, `get_` and `list_` methods for `wastage_reason`, `labour_role` and `measurement_type` (plural lists). Creates and updates accept JSON attribute objects (`&serde_json::Value`); results are JSON objects, retaining Bilimbi's map-shaped contract. Lists take an optional active filter and sort by code. These reads require `configuration.view`; writes require `configuration.manage` inside the mutation. Codes are trimmed, uppercased and immutable; labels and active status can change. Measurement types carry immutable `value_type` (`string`, `integer`, `decimal`, `boolean`) and optional numeric `unit` label, with mutable minimum, maximum and target. Limits have at most 12 decimal places and absolute values below 10¹². Minimum cannot exceed maximum; target need not be in range.

Capture methods take `(scope, company, target_id, &attrs)` and return JSON objects. Lists take `(scope, company, target_id)` and retain all historical rows with nullable `corrected_by_id`. Quantities and limits are exact decimal strings; timestamps are RFC3339 strings; absent values are JSON null. Tenant/company ownership and retry fingerprints stay private. Recorder and stopper identity come only from Scope. A recorder must belong to the target company; impersonation refuses with `Capture refused under impersonation`. Agents retain their acting-for user. Each capture checks its capability before the transaction (preserving refusal decisions) and again inside it. Grant capture capabilities explicitly: they are not added to `tenant_owner`, matching Bilimbi.

| Method | Target and attrs | Capability suffix |
| --- | --- | --- |
| `record_wastage` | Run; request/reason/item/location/quantity/observation, optional identity/note/occurred_at | `wastage.record` |
| `correct_wastage` | Previous capture; request/reason for correction/absolute nonnegative quantity, optional reason_id/note | `wastage.correct` |
| `clock_in` | Order; request/role, optional worker/run/note; server time only | `labour.record` (own), `labour.manage` (others) |
| `clock_out` | Entry; no attrs or retry key; server time and stopper | Same ownership rule |
| `record_labour` | Order; request/role/worker/run/note plus past started_at/stopped_at, stop strictly after start | Same ownership rule |
| `correct_labour` | Previous entry; request/correction_reason, optional role/run/note/start/stop; omitted retains, null stop reopens | `labour.manage` |
| `record_measurement` | Run; request/measurement_type_id/value, optional output identity/note/measured_at | `measurement.record` |
| `correct_measurement` | Previous measurement; request/correction_reason/value, optional note | `measurement.correct` |

Suffixes are under `factory.production-execution.`. Attr names are Bilimbi's: `request_id`, `reason_id`, `item_id`, `location_id`, `quantity`, `observation`, `identity_id`, `role_id`, `worker_user_id`, `execution_id`, `note`, `correction_reason`, `started_at`, `stopped_at`, `occurred_at`, `measured_at`. Own labour defaults to the Scope user; an agent explicitly names a worker and needs manage. Worker assignments use Core User's company-accounts read on the mutation connection. Current intervals can touch but cannot overlap across any orders. The writable-company lock serializes capture writers, preventing concurrent open clocks and branched corrections. `list_labour` and `labour_summary` target the order; the summary excludes superseded rows and totals closed seconds by worker and nullable run, with open entries separate.

Wastage must name a stock item/identity in the run and uses its native unit. Its row and Inventory production consumption commit together. Capture corrections point to their immediate predecessor; Inventory corrections post `previous − replacement` against the root consumption. Zero voids wastage; unchanged quantity appends evidence without a posting. Inventory retry keys use `pe-wastage:`, distinct from XP1's unrestricted `pe:` namespace. Event times are no earlier than run start and no later than now, not bounded by completion.

Measurements use Inventory's property-value validator and snapshot unit/limits/target. Values below minimum or above maximum flag out of range; endpoints are valid. Nonnumeric types or no bounds give null (target alone never flags). Corrections preserve original snapshots, including after deactivation. Non-null identity must name a positive stock output of the run.

Every request ID is company-local within its capture table, nonblank and at most 240 bytes. SHA-256 fingerprints cover action, target and the normalized command, including resolved correction defaults, excluding generated times and recorder identity. Retries revalidate configuration and worker eligibility, then return existing evidence without a new audit action; conflicting content refuses. Corrections have one successor and may themselves be corrected, always with a reason. PostgreSQL refuses UPDATE/DELETE/TRUNCATE of wastage and measurements; labour permits only a one-time close. Every saved row returns a complete audit `Change`.

## Traces and yield

`trace_backward` and `trace_forward` enrich Inventory genealogy with runs, order revisions, resource, times and source. Forward traces include draws of every visited identity, retaining consumption-only runs. `get_run_yield` returns exact native-unit input/product/trim/waste/wastage/variance totals and correction posting IDs. Wastage adjusts the original material side and increases recorded waste; waste includes wastage. Arithmetic stays in PostgreSQL over Inventory facade results, never sibling SQL or Decimal native totals. `get_unit_yield` returns whole-run balances plus identity-specific input/output, not invented allocations. Cross-unit agreement passes through Inventory's recorded-conversion balance. No yield percentage is invented.

## Runtime proof

Run the mounted Platform `scripts/domain-ci.sh` on an empty PostgreSQL 18
database, then on a separate freshly migrated disposable database the
container walkthrough's `production-execution` binary (root README). It drives
the registered facades as real sessions: orders and refusals, live and import
completion in all three posting shapes, idempotent and concurrent retries,
atomic rollback when Inventory or the execution record fails, hold rules and
overrides (denied, impersonated, live and imported), capability and tenant
isolation, SQL immutability and audit records.

Then run `shop-floor` on that same disposable database:

```sh
BILIBI_FACTORY_FIXTURE=disposable CARGO_TARGET_DIR="$HOME/bilibi/target" \
  cargo run --manifest-path fixtures/walkthrough/Cargo.toml --bin shop-floor
```

The XP2 executable checks code normalization/immutability/deactivation, wastage delta chains and stock/yield, atomic rollback after posting, range snapshots and output identity validation, own/supervisor clocks and corrections, cross-order overlap/adjacency/reopening, inactive worker refusal, concurrent retries, impersonation/company/tenant refusals, SQL immutability and audit counts. It installs no product data or routes.


## Shop floor

`/factory/floor` (Production → Shop floor) requires `factory.production-execution.floor.view`. Pick an order, then a run; the `order` and `run` query parameters restore the selection. XP3 renders native-unit run yield, wastage and output measurements with their correction chains and range flags. Recording and correcting are separate grants, rechecked by every write-shaped event and again by the facade. Draft request IDs survive validation refusals and duplicate submissions. Labour is available on the selected order before or after selecting a run: clock in/out, closed-time totals per run, supervisor worker selection, finished entries and correction chains. `labour.record` controls own clocks and `labour.manage` controls other workers, finished entries and corrections.


Timestamps use Base Datetime's display policy and shared timestamp component. `src/web/time.rs` is the private company-time entry contract: company/local display reads entered wall time in the company zone; UTC display reads UTC. A daylight-saving gap is refused and a fold picks the earlier instant. An unchanged minute input keeps the original seconds and subsecond precision.

### Page proof

On a disposable migrated database, run the existing `production-execution` and `shop-floor` walkthrough binaries, then `floor-pages` from `fixtures/walkthrough` with the same `BILIBI_FACTORY_FIXTURE=disposable`, `DATABASE_URL`, and Platform `CARGO_TARGET_DIR`. It drives the registered PageService and real facade mutations (selection, range flags, both corrections, retry, stale permission and archived company refusals), plus the same company-time source. `BILIBI_FLOOR_SERVE=yes APP_ORIGIN=http://localhost:35593 BIND_ADDR=127.0.0.1:35593` keeps the composed app running for browser proof after the checks. No fixture data is a product seed.


### Configuration pages

`/factory/wastage-reasons`, `/factory/labour-roles` and `/factory/measurement-types` sit under Factory administration and require `configuration.view`. They share the private `src/web/codes.rs` adapter with FlexTable/ListColumns, new/edit forms and activate/deactivate actions. Every write-shaped event rechecks `configuration.manage`; the facade checks again in its mutation. Code, measurement value type and unit stay fixed while labels and measurement limits can change. New limits affect new measurements only. Form submits are named `save_entry` because the Platform reserves `save` for Company edits.

Measurement type limits use normalized decimal text in the list and editor, matching Bilimbi's `limit/1`. Formatting uses the exact stored text through `src/web/shared.rs`.

Run `labour-pages` after `floor-pages` on the same disposable database for XP4. It exercises the registered page adapter with real sessions: all three configuration create/edit/activation flows, immutable-field forgery, restricted/stale grants, own and supervisor clocks, retry, finished intervals in company time, corrections and recomputed totals, malformed time and archived company refusals. Set `BILIBI_FLOOR_SERVE=yes APP_ORIGIN=http://localhost:35594 BIND_ADDR=127.0.0.1:35594` to keep that app running for browser proof.

## Reviewed production operations (H6)

H6 adds headless order creation, live/import completion and hold-override reads through the Platform agent registry and reviewed safe-write plans. It retains the facade's existing capability checks and private Inventory posting authority; no page or new capability is introduced. Prerequisites that need generated IDs (a new order or output identity) must be committed in a separate plan before referring to them. Plans commit atomically but preview each operation against committed data, not earlier speculative operations.

| Operation under `factory.production_execution.` | Access |
| --- | --- |
| `order.list`, `order.get` | `floor.view` |
| `execution.list`, `execution.get`, `hold_override.list` | `floor.view` |
| `order.create`, `operation.complete` | Reviewed safe write; `configuration.manage` and the facade's sibling grants |

The searchable guide `factory.production_execution` documents inputs and workflow. Both HTTP (`/api/v1/operations`, `/api/v1/plans`, `/api/v1/plans/commit`) and MCP expose the same registry. Direct writes are refused. Every input includes `company_id`; reads take `order_id` or `execution_id` as applicable and lists accept an optional `limit` bounded by the facade. Quantities are exact decimal strings, timestamps are RFC3339 with offsets. Pre-1970 completion/override timestamps are refused because the existing retry fingerprint cannot serialize them.

`order.create` takes the `NewOrder` fields above. Commit it before using the returned ID in a completion plan. `operation.complete` takes `order_id`, `source` (`live` or `import`) and the `Completion` fields above. Omitted `unit_id` uses the immutable native unit; every non-native line must supply `conversion_version`, so publishing a newer conversion cannot silently change an approved draw. Output lines may create an identity (`kind`, `code`, optional `dimensions`); previews use symbolic identities, never predicted IDs, and commits return resolved IDs in signed posting entries. Record the order/identity in an earlier plan before citing its generated ID.

Review executes the same owner validation/posting path in a rolled-back `MutationBoundary`; it leaves no business rows or mutation audits, although PostgreSQL sequences may have gaps. Import and override preflight authorization decisions remain durable on refusal. Commit repeats all validations under the writable-company lock, retains request-id replay semantics, and derives recorder/authority only from Scope/the private contributed static. Import and override grants are rechecked, not inferred from operator or historical approver fields. An identical recorded retry does not reauthorize historical holds; import still requires its grant. Platform authorization has no transaction-wide grant-revocation fence: this does not promise revocation cannot race the final check.

Run the H6 proof on a **freshly migrated separate disposable database**:

```sh
DATABASE_URL=postgres://bilibi:bilibi@127.0.0.1:5438/<disposable-db> \
  BILIBI_FACTORY_FIXTURE=disposable CARGO_TARGET_DIR="$HOME/bilibi/target" \
  cargo run --manifest-path fixtures/walkthrough/Cargo.toml --bin production-operations
```

To exercise HTTP/MCP too, start `bilibi-web run` against that same database and set `BILIBI_FACTORY_API` to its local address when running the proof. The executable installs only disposable fixture accounts, creates actual units/materials/definitions/stock, reviews and commits production operations, and checks generated-identity/conversion evidence, import/override denial logs and revocation, replay/conflict, tenant isolation, malformed inputs, single-use plans, and whole-plan rollback after a later overdraw. The optional edge checks obtain a real device-grant token, create/complete through HTTP and consume through MCP with an authenticated agent recorder; tokens and plan codes are never printed. It adds no production routes, accounts or data.
