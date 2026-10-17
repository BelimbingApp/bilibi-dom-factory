# factory/inventory

Factory Inventory (`bilibi-factory-inventory`), ported from Bilimbi's
`Bilimbi.Factory.Inventory`. It holds the module's descriptor, vocabulary,
menu and settings (slice X0), and behind one facade the units, stock
locations and item settings (X0) and the item catalog: the item master,
material types with property definitions, materials with a fixed native unit
and immutable conversion versions (XI1), and the Material Transaction ledger
(XI2). Genealogy, production posting authority and the pages follow in XI3
and XI4.

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
| `get_transaction`, `list_transactions`, `get_stock_position` | `…configuration.view` | transaction entries and current stock; lists newest ID first, optionally by item, default 100 / maximum 500 |

A write also needs the company as an authorized target (the actor's own
company, or `admin.company.tenant-wide.manage`) and a company that is not
archived; it runs in one `MutationBoundary` transaction, so each change is
audited under its table name, and the capability is checked again inside the
transaction. A record of another company is reported as not found. Failures are
`Error`: its texts are Bilimbi's ("Unit not found", "Code has already been
taken", "Invalid item statuses", "Material type in use").

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
  time comes from the database after locks. `shipment` and `destination`
  are stored opaque references. Production context (`operation_execution`,
  `order_or_batch`, `work_centre`) and corrections of authority-backed
  transactions are refused with `Unregistered posting authority` until the
  production posting functions in XI3. No cross-domain lookup occurs.
- **Database backstops.** Migration `factory-inventory-003-ledger` creates
  transactions and signed entries with company-local composite references,
  append-only UPDATE/DELETE/TRUNCATE triggers and deferred checks on both
  tables. At commit every transaction must have entries and sum to zero
  separately in each native unit. Receipts/consumption/corrections pair stock
  with an opposite boundary entry; transfers pair two stock locations.

These operations have no pages in Bilimbi. XI2 adds neither pages nor agent
operations. The disposable walkthrough drives the facade, concurrent posts,
retries, permissions, retirement and PostgreSQL refusals. Genealogy, identity
stock, production postings and transaction balance/correction-chain reports
belong to XI3.

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
`factory.inventory.configuration.view`. The pages arrive in XI4; until then
those two links have no page behind them.

## Schema

Migration `factory-inventory-001-units-locations`: `factory_inventory_units`
(with retirement) and `factory_inventory_locations`, each keyed to a company of
the row's tenant, with a unique code per company. A row keeps its company and
code for good, and a retired unit stays retired, enforced by trigger.

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
