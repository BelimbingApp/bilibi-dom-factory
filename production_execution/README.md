# Production Execution

`ProductionExecution` owns production orders, completed operation executions
and material hold overrides. Resolve `SERVICE`
(`factory/production_execution.production_execution`) from the service
registry. This is XP1; shop-floor capture (wastage, labour, measurements, yield
and traces) is XP2 and the pages and menu entries follow in XP3/XP4. It mirrors
Bilimbi `ProductionExecution` at `f39ed3dcf2ce2694d30c2b88e82212780f4f3b98`.

## Facade

Every read takes a sealed scope and company, requires
`factory.production-execution.floor.view`, and checks the live company. Every
write rechecks `factory.production-execution.configuration.manage` and the
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

Three tables with tenant and company ownership; product, resource and Inventory
references are opaque IDs the sibling facades validated (no SQL dependence on
their tables, no cross-container reference). PostgreSQL refuses UPDATE, DELETE
and TRUNCATE on executions and overrides. Every saved row is audited with
complete old/new data. There are no production seeds, agent operations, CSS or
JavaScript.

## Runtime proof

Run the mounted Platform `scripts/domain-ci.sh` on an empty PostgreSQL 18
database, then on a separate freshly migrated disposable database the
container walkthrough's `production-execution` binary (root README). It drives
the registered facades as real sessions: orders and refusals, live and import
completion in all three posting shapes, idempotent and concurrent retries,
atomic rollback when Inventory or the execution record fails, hold rules and
overrides (denied, impersonated, live and imported), capability and tenant
isolation, SQL immutability and audit records.
