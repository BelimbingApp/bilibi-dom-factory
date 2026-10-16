# factory/inventory

Factory Inventory (`bilibi-factory-inventory`), ported from Bilimbi's
`Bilimbi.Factory.Inventory`. This slice (X0) holds the module's descriptor,
vocabulary, menu and settings, and the units, stock locations and item
settings behind one facade. The item catalog, material types, the Material
Transaction ledger, genealogy, production posting authority and the pages
follow in slices XI1 to XI4.

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

A write also needs the company as an authorized target (the actor's own
company, or `admin.company.tenant-wide.manage`) and a company that is not
archived; it runs in one `MutationBoundary` transaction, so each change is
audited under its table name, and the capability is checked again inside the
transaction. A record of another company is reported as not found. Failures are
`Error`: its texts are Bilimbi's ("Unit not found", "Code has already been
taken", "Invalid item statuses").

## Capabilities

Seven keys, verbatim from Bilimbi, all granted to `tenant_owner`: the five
`commerce.inventory.item.*` keys are Belimbing's durable item master keys
(`create`, `list`, `update`, `view`, plus `commerce.inventory.manage`); the
other two are `factory.inventory.configuration.view` and `.manage`. The item
keys gate the item catalog that XI1 adds.

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
code for good, and a retired unit stays retired, enforced by trigger. Money and
quantity columns arrive with the ledger.
