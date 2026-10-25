# factory/inventory

Factory Inventory (`bilibi-factory-inventory`), ported from Bilimbi's
`Bilimbi.Factory.Inventory`. It holds the module's descriptor, vocabulary,
menu and settings (slice X0), and behind one facade the units, stock
locations and item settings (X0) and the item catalog: the item master,
material types with property definitions, materials with a fixed native unit
and immutable conversion versions (XI1), the Material Transaction ledger
(XI2), and lot and unit genealogy with transforms, balances and the
production posting authority (XI3). The pages are XI4 (below).

## Owner contract

The facade is `Inventory`, resolved as the service `factory/inventory.inventory`
(`bilibi_factory_inventory::SERVICE`). Every operation takes the sealed `Scope`
and a company ID; the company must be live and inside the scope's tenant.

| Operation | Needs | Notes |
| --- | --- | --- |
| `item_settings`, `item_setting_overrides` | `factory.inventory.configuration.view` | effective settings (company, then tenant), and the company's own overrides |
| `configure_item_settings` | `…configuration.manage` | statuses and default currency as one write; `None` removes an override |
| `list_units`, `get_unit` | `…configuration.view` | ordered by code; limit 100 by default, 500 at most |
| `create_unit`, `rename_unit`, `retire_unit` | `…configuration.manage` | a code is unique per company and stable; a retired unit takes no rename or second retirement |
| `list_locations`, `get_location` | `…configuration.view` | ordered by code |
| `create_location` | `…configuration.manage` | the code is trimmed and upper-cased |
| `list_items` | `commerce.inventory.item.list` | ordered by SKU, optionally of one status |
| `get_item`, `get_item_by_sku` | `commerce.inventory.item.view` | a SKU is looked up trimmed and upper-cased |
| `create_item` | `commerce.inventory.item.create` | Belimbing's create rules and the company's item settings (below) |
| `list_material_types`, `get_material_type` | `…configuration.view` | ordered by code |
| `create_material_type`, `update_material_type`, `retire_material_type` | `…configuration.manage` | code upper-cased and unique per company; definitions fixed once a material uses the type |
| `list_materials`, `get_material` | `…configuration.view` | by item; listed by SKU |
| `register_material` | `…configuration.manage` | an existing item, once, in an active native unit, with an optional active type and its values |
| `create_material` | `…configuration.manage` and `commerce.inventory.item.create` | the item and its material in one write |
| `update_material` | `…configuration.manage` and `commerce.inventory.item.update` | the item's title, description, status, notes and currency, and the values for its type, replaced whole |
| `retire_material` | `…configuration.manage` | keeps its history readable |
| `define_conversion` | `…configuration.manage` | the next version of one unit's factor to the native unit |
| `list_conversions`, `get_conversion` | `…configuration.view` | every version by unit code then version; the current one, or a named version |
| `record_receipt`, `record_transfer`, `record_consumption`, `record_correction` | `…configuration.manage` | append-only, audited, idempotent postings; actors come from `Scope` |
| `record_output`, `record_transform`, `record_production_consumption`, `record_production_correction` | `…configuration.manage` and a registered `PostingAuthority` | the four production postings (see "Posting authority") |
| `get_transaction`, `list_transactions`, `get_stock_position` | `…configuration.view` | transaction entries and current stock; lists newest ID first, optionally by item, default 100 / maximum 500 |
| `list_corrections`, `get_transaction_balance` | `…configuration.view` | corrections of a transaction (and of those), and its balance net of them |
| `get_identity`, `get_identity_positions`, `trace_backward`, `trace_forward`, `list_identity_draws` | `…configuration.view` | lot and unit genealogy |
| `posting_authority_registered` | none | whether a reference is a contributed authority |

A write also needs the company as an authorized target (the actor's own
company, or `admin.company.tenant-wide.manage`) and a company that is not
archived; it runs in one `MutationBoundary` transaction, so each change is
audited under its table name, and the capability is checked again inside the
transaction. A record of another company is reported as not found. Failures are
`Error`: its texts are Bilimbi's ("Unit not found", "Code has already been
taken", "Invalid item statuses", "Material type in use").

## Agent operations

Locations, receipts, transfers, consumption and corrections have no page in
the reference, so a fresh install reaches them through Bilibi's agent API
(`/api/v1`, `/mcp`), contributed from `src/agent_operations.rs` as
`factory.inventory.*` operations. Every input names a `company_id`; quantities
and conversion factors are decimal strings and results carry exact decimal
strings, never floats. Reads need `factory.inventory.configuration.view`,
writes `factory.inventory.configuration.manage`; each calls the facade's own
code, so company authorization, the writable-company lock and refusal texts
are the facade's. Every operation serves every door (`channels: every`) and
lists no step-up: Factory declares no Base Step-up operation, as Bilimbi has
none.

| Key | Kind | Does |
| --- | --- | --- |
| `location.list`, `location.get` | read | the facade's `list_locations`, `get_location` |
| `stock.position` | read | `get_stock_position`: exact stock of an item at a location |
| `transaction.list`, `.get`, `.corrections`, `.balance` | read | `list_transactions` (newest first, `item_id`, `limit`), `get_transaction`, `list_corrections`, `get_transaction_balance` |
| `identity.get`, `.positions`, `.draws`, `.trace` | read | `get_identity`, `get_identity_positions`, `list_identity_draws`, and `trace_backward`/`trace_forward` by `direction` |
| `location.create` | safe write | `create_location` |
| `receipt.record`, `transfer.record`, `consumption.record`, `correction.record` | safe write | the facade's four ordinary postings, with weighed receipts, lot or unit identities (receipt lines), identity citations and signed corrections |

A write is a reviewed safe write: `preview` and `commit` through
`base/agent_api.plans`, never a direct call. The preview runs the real code
path (`ledger::post`, `locations::create`) in a transaction of its own and
rolls it back, so the review shows what the commit will decide: every refusal
(blank fields, insufficient stock by location, identity or unidentified pool,
a retired material, an unknown conversion, a `request_id` reused with other
content), each entry with its exact native quantity, and the stock before and
after. It carries no generated ID or timestamp, so the commit's own preview
reproduces it and any stock change in between makes the plan stale. A request
that repeats a recorded one reviews as `replay: true` and commits as the
original. The commit posts inside the plan's audited transaction; the
capability and company are checked again in the facade. A plan's reviews see
committed data only, so create a location in its own plan before a posting
names it.

Production postings (`record_output`, `record_transform`, the production
consumption and correction) are deliberately not operations: they need a
registered `PostingAuthority`, which an agent cannot hold. Production context
keys are not in the schema, and correcting a production transaction is refused
as `Unregistered posting authority`.

## Catalog rules

- **Item master.** `commerce_inventory_items` keeps Belimbing's columns and
  table name. On create the SKU and currency are trimmed and upper-cased, a
  blank storage location is none, and the quantity on hand defaults to one
  (0 to 999999). An omitted status is the configured vocabulary's first entry
  and any other must be in it; unconfigured, a non-blank status is required.
  An omitted currency is the configured default; unconfigured, it is
  required. The SKU never changes. `category_id` and `product_template_id`
  are opaque references to a Commerce Catalog Inventory does not own, read
  but never written. `quantity_on_hand` and `storage_location` are the item
  master's own values, never stock positions.
- **Material types.** A definition is `PropertyDefinition { key, label,
  value_type, unit, required }`: a key `[a-z][a-z0-9_]{0,63}` unique in the
  type, a label of at most 255 characters, a value type from `VALUE_TYPES`
  (`string`, `integer`, `decimal`, `boolean`), and a unit label (at most 32)
  only on a numeric type; anything else is "Invalid property definitions".
  Definitions may change until a material uses the type ("Material type in
  use"); the code and name may change later too. Which types exist is the
  company's configuration.
- **Material identity.** An item becomes a material once ("Item has already
  been taken"), in a native unit that never changes. A material may take an
  active type and values for its definitions: no undefined key, every
  required key present, each value of its type (a string trimmed, at most
  1000 characters; a decimal from a JSON integer or decimal string, kept at
  the precision given), else "Invalid properties". A material without a type
  holds none.
- **Conversions.** One conversion unit equals `factor` native units, a
  positive decimal with at most 12 decimal places and 12 integer digits. A
  version never changes; defining a factor again is the next version, and the
  native unit or a retired unit takes none ("Native unit", "Unit retired").
- **Retirement.** Retiring keeps every reference readable. A retired unit
  takes no new material or conversion, and a unit that is an active
  material's native unit cannot retire ("Unit in use"). A retired type takes
  no edit and no new material. A retired material takes no update and will
  refuse new postings. Retirement refuses any non-zero stock position;
  corrections may still adjust a retired material, as in Bilimbi.

## Material Transaction ledger

`PostingRequest` carries a company-unique `request_id`, source `evidence`,
optional `effective_at`, optional opaque `context`, and non-empty `lines`.
Use `StockLine` with an item, location, quantity and `Observation` (`Measured`,
`Declared`, `Counted`, `Derived`). A transfer uses `from_location_id` and
`to_location_id` instead. Corrections need `corrects_transaction_id` and
`reason`; their quantities are signed adjustments, while other postings
require positive quantities. The original transaction never changes.

- **Recorded and native quantities.** A quantity accepts at most 12 decimal
  places and 12 integer digits. An optional `unit_id` uses the latest
  conversion, or `conversion_version` pins an earlier one. Each stock entry
  retains the observed quantity, unit and conversion ID/version. Native
  quantities are computed and rounded to 12 places in PostgreSQL. They and
  stock-position totals are **exact decimal strings** in read models:
  `numeric(36,12)` exceeds `rust_decimal`'s 96-bit mantissa. Do not parse them
  as floats. Recorded quantities and conversion factors remain `Decimal`.
- **Weighed receipts.** `ReceiptMeasurement` records supplier-declared net,
  measured gross, tare, net, unit and weighing-point reference. Gross minus
  tare must equal net; exactly one measured line must explicitly name the
  same unit and quantity. `supplier_variance()` is declared minus measured
  net. The ticket and source evidence remain attached to the transaction.
- **Retries.** Canonical decimal values, the kind, actor and request make a
  SHA-256 fingerprint. An omitted effective time stays omitted in this
  fingerprint. A matching retry returns the original, including its original
  conversion and timestamps, without adding entries or mutation audit rows.
  A different request under the same ID returns `Request id conflict`.
  Replays still require current write permission and a writable company.
- **Stock and locking.** The Core writable-company lock serializes writes,
  including conflicting request IDs that name disjoint materials; material
  locks also serialize retirement and conversion changes. Before inserting,
  every position must cover all withdrawals in the request. Its own additions
  cannot fund them. Stock is the sum of signed stock entries, never the item
  master's `quantity_on_hand`.
- **Time and context.** Effective time cannot be in the future; recorded
  time comes from the database after locks. Context references are stored
  opaque, and no cross-domain lookup occurs. `shipment` and `destination`
  are open to any posting; production context (`operation_execution`,
  `order_or_batch`, `work_centre`) needs a production posting (below).
- **Database backstops.** Migration `factory-inventory-003-ledger` creates
  transactions and signed entries with company-local composite references,
  append-only UPDATE/DELETE/TRUNCATE triggers and deferred checks on both
  tables. At commit every transaction must have entries and sum to zero
  separately in each native unit. Receipts/consumption/corrections pair stock
  with an opposite boundary entry; transfers pair two stock locations.

- **Transforms.** `record_transform` takes `inputs` drawn from stock and
  `outputs` put into it (no `lines`), and commits them with a genealogy link
  from every input entry to every output entry. An output may carry an opaque
  `output_role`; other lines drop it. Observed quantities are never adjusted
  and nothing is converted between units: each native unit balances on its
  own, and one whose inputs and outputs differ records the difference (input
  less output) as a `variance` entry in that unit. That needs `Variance`
  evidence, `Shared { evidence, reconciliation_basis }` for every differing
  unit or `Units` naming each differing unit's own; a differing unit without
  it is `Variance required`, and evidence no difference uses (shared with
  nothing differing, or naming a unit that does not differ) is `No variance`.
- **Balances.** `get_transaction_balance` reads a transaction net of its
  corrections (each correction entry nets into the side of the transaction's
  first entry for the same material and identity). `per_unit` gives each
  native unit, in first-seen order, its observed input, output, difference
  and recorded variance. `cross_unit` exists only when the stock entries span
  several native units, and only in a unit every line was posted in, natively
  or as its recorded unit; a recorded line counts at its recorded quantity
  and names the conversion (item, ID, version, factor) it was posted through,
  so a later conversion version never restates it. Every sum is computed in
  PostgreSQL and returned as exact decimal text.

These operations have no pages in Bilimbi, and XI2 and XI3 add neither pages
nor agent operations. The disposable walkthrough drives the facade, concurrent
posts, retries, permissions, retirement and PostgreSQL refusals, and its
`genealogy` binary the XI3 behaviour below.

## Lot and unit identities

A receipt, production output or transform output line may create an identity with `identity: NewIdentity { kind: Lot | Unit, code, dimensions }`. The code is unique for that material in the company ("Code has already been taken") and never changes. A later transfer, consumption, correction, production output or transform line cites it by `identity_id`. **An output citing an existing identity adds its quantity to that identity's stock**; it does not replace a previous posting or change the identity's code, kind, dimensions or original source transaction. Output and transform postings still require a registered production authority.

When a transform uses any identity every input and output must be identified (`Identity required`). Creating one on a draw, citing one on a receipt, or specifying both `identity` and `identity_id` on one line is `Invalid identity`. An identity of another material, company or tenant is `Identity not found`. Before a draw Inventory checks the location's whole stock (`Insufficient stock`), then the cited identity's stock there (`Insufficient identity stock`), or else the unidentified pool (`Insufficient unidentified stock`). Additions in the same posting cannot cover its withdrawals. Transform links include outputs citing existing identities, so traces retain the additional inputs.

This intentionally extends Bilimbi `Inventory.Ledger.validate_identities`, which refuses an existing identity on a positive output. It uses the same locked planning, ownership checks, append-only entries, genealogy and audit path; no schema change or identity merge is needed.

A unit (not a lot) may carry one to three `dimensions`, `width`, `length` and
`thickness`, each a positive `value` of at most 12 decimal places, a `unit`
(`um`, `mm`, `cm`, `m`) and a `provenance` (`measured`, `nominal`), kept on the
immutable identity. `get_identity` returns its kind, code, item, source
transaction and dimensions; `get_identity_positions` its positive balances by
location, empty once fully drawn. `trace_backward` and `trace_forward` walk
transform links across any number of transforms and return the root, every
visited identity, the `(input identity, output identity, transaction)` links
and the receipts among the identities' source transactions.
`list_identity_draws` lists the consumptions and transforms that drew the
identity, including consumption that created no link. Unidentified material
remains supported.

## Import posting access

A production module can opt its private registered `PostingAuthority` into its own `<module>.import` capability with `with_import_capability`. Boot refuses a capability outside that module's namespace. `production_import(scope, company, authority)` seals access for one scope, company and registered authority; it caches no grant. Every use rechecks authorization on the caller's connection.

`record_import_on` accepts only completion inputs and outputs, choosing production consumption, output or transform and creating their lot/unit identities through the same ledger path. Configuration, ordinary postings and production corrections retain their existing gates. `get_import_identity_on` and `get_import_transaction_on` read the immutable evidence an import validates or returns. Product Definition accepts the sealed access only for exact revision and routed resource reads. The owning Production Execution import facade is documented in [`production_execution/README.md`](../production_execution/README.md#imports).

## Posting authority

Receipts, transfers, ordinary consumption and corrections are open to any
caller with the capability and refuse production context, and a correction
of a transaction posted by an authority, as `Unregistered posting authority`.
Production goes through `record_output`, `record_transform`,
`record_production_consumption` and `record_production_correction`, each
taking `&'static PostingAuthority`. Bilimbi declares the authority in OTP
metadata; here the declaring module contributes it at link time:

```rust
static AUTHORITY: PostingAuthority = PostingAuthority::declare("factory/production_execution");
inventory::submit! { Contribution::new(POSTING_AUTHORITIES, "factory/production_execution", provenance!(), &AUTHORITY) }
```

Boot refuses a declaration from a module outside the `factory` container, or
one whose contribution ID or declared module is not the contributing module.
Inventory names no authority and does not depend on Production Execution. It
reads the registry once when its service starts and accepts only a reference
to a contributed static (pointer identity), so an equal value built elsewhere
is refused; keep the static private. Each transaction records the module in
`posting_authority` (Bilimbi records the Elixir module name). Production
Execution contributes the production authority (XP1); without it mounted
nothing is registered and every production posting is refused.

Bilimbi's callers post inside their own `Repo.transaction`, so a module whose
record must commit with a posting uses the `_on` variants
(`record_output_on`, `record_transform_on`, `record_production_consumption_on`,
`record_production_correction_on`). They take the caller's open
`MutationTransaction` and run the same checks, locks and idempotency as the
ones that begin their own; the caller chose the audit action and commits or
rolls back. Production Execution completes an operation this way.

## Capabilities

Seven keys, verbatim from Bilimbi, all granted to `tenant_owner`: the five
`commerce.inventory.item.*` keys are Belimbing's durable item master keys
(`create`, `list`, `update`, `view`, plus `commerce.inventory.manage`); the
other two are `factory.inventory.configuration.view` and `.manage`. The item
keys gate the item master; configuration gates units, locations, settings,
material types, materials and conversions, and a write that touches both
needs both. Bilimbi's facade checks no capability (its pages check
configuration only), so these facade checks are the Rust port's.
`commerce.inventory.manage` is declared for grants in adopted databases and
gates nothing yet.

## Sibling callers

`get_item_on`, `get_material_on` and `get_unit_on` read through a caller's
PostgreSQL client/transaction with the same capability and scope checks as the
pooled reads. Use them while holding a mutation's company lock; acquiring a
second connection could exhaust the pool with concurrent writers.
`normalize_property_definitions` and `validate_property_values` expose the
shared property contract through Inventory, without exposing private helpers.

## Settings

| Key | Scopes | Default |
| --- | --- | --- |
| `factory.inventory.item_statuses` | tenant, company | none: any non-blank status is accepted |
| `factory.inventory.default_currency_code` | tenant, company | none: each item names its own currency |

The vocabulary and the currency are a company's configuration, never
Inventory's. Statuses are an ordered list of distinct, unpadded, non-blank
strings (the first is the default); a currency is three upper-case letters.

## Menu

The root `admin.factory` ("Factory", under Administration) and its items Units
(`/factory/units`) and Materials (`/factory/materials`), each needing
`factory.inventory.configuration.view`.

## Pages

Service `factory/inventory.pages`; every route needs
`factory.inventory.configuration.view`, checked on each request and live event,
and is framed-capable. Each page is a mounted state over the facade; every
write-shaped event checks `…configuration.manage` again before the facade does,
and the facade a third time.

| Route | Operation | What it does |
| --- | --- | --- |
| `/factory/units` | `units` | list; New unit, Edit (name only; the code is fixed), Retire |
| `/factory/material-types` | `material_types` | list; New type, Edit, Retire; property definitions are edited as JSON |
| `/factory/materials` | `materials` | list; New material (item, native unit, type, JSON values), Edit, Retire |
| `/factory/conversions` | `conversions` | materials table, then the picked material's versions table and the Publish conversion form |
| `/factory/item-settings` | `item_settings` | the company's own statuses and default currency, beside the effective values |

Lists are `FlexTable` over built-in `ListColumns` (column settings, drag
reorder, row height, compact rows). Bilimbi's tables have no sort, filter or
pager, so these have none. Bilimbi's `data-confirm` on Retire is a
`ConfirmDialog` ("Retire this unit?", "…material type?", "…material?"). Flash
and alert texts, element IDs and labels are Bilimbi's; an actor without a
company reaches the facade's "Company not found".

## Schema

Migration `factory-inventory-001-units-locations`: `factory_inventory_units`
(with retirement) and `factory_inventory_locations`, each keyed to a company of
the row's tenant, with a unique code per company. A row keeps its company and
code for good, and a retired unit stays retired, enforced by trigger.

Migration `factory-inventory-004-genealogy`: `factory_inventory_identities`
(kind, code unique per material, dimensions checked to `width`/`length`/
`thickness` on a unit only) and `factory_inventory_genealogy_links`, both
append-only, and `identity_id` on transaction entries. An entry's identity
must be of its own material and only a stock entry names one; a link's input
and output entries must belong to its own transaction. All by composite keys.

Migration `factory-inventory-002-catalog`: `commerce_inventory_items`,
`factory_inventory_material_types`, `factory_inventory_materials` and
`factory_inventory_unit_conversions`. Every reference is composite with
`company_id`, so a row only names records of its own company. Triggers keep an
item's company and SKU, a material's item, native unit and type, and keep
anything retired retired; a check keeps a material without a type free of
values. Conversion rows are append-only (UPDATE, DELETE and TRUNCATE are
refused) and never name the material's native unit. The adopted Belimbing
defaults for status (`draft`) and currency are not carried: both come from
the company's settings or the request.
