# Product Definition

`ProductDefinition` owns resources, products and immutable formula/routing
revisions. Resolve `SERVICE` (`factory/product_definition.product_definition`)
from the service registry. This is XD1; pages and menu entries arrive in XD2.
It mirrors Bilimbi `ProductDefinition` at
`f39ed3dcf2ce2694d30c2b88e82212780f4f3b98`.

## Facade

Every read takes a sealed scope and company, requires
`factory.product-definition.configuration.view`, and checks the live company.
Every write rechecks `.manage` and the authorized writable company inside
`MutationBoundary`. Both capabilities belong to `tenant_owner`.

- `create/get/list_product(s)`: one product per Inventory item, unique company
  code. Codes retain case. Item existence is checked through Inventory.
- `create/update/retire/get/list_resource_type(s)`: uppercase trimmed code and
  company-defined properties using Inventory's shared property mechanism.
  Definitions freeze after any resource uses them, including retired resources.
  Retirement requires no active resources.
- `create/update/retire/get/list_resource(s)`: a fixed resource type and typed
  properties. Updates replace supplied properties; omission preserves them.
  Retired resources and types cannot be edited or retired again.
- `publish_formula`: JSON `{lines: [...], process_config: {...}}`. Each line has
  positive `item_id`, `unit_id`, `quantity`, and `role` (`input`/`output`). Optional
  `output_role` is a nonempty output-only string; `material_hold_rule` is an
  input-only object. There must be an output of the product's item.
- `publish_routing`: JSON `{operations: [...], process_config: {...}}`. Each
  operation has unique nonempty `code`, unique positive `sequence`, `inputs`
  (possibly empty), nonempty `outputs` and `allowed_resource_ids`. ID lists hold
  positive integers. Input order is retained, not sorted by sequence.
- `get/list_formula_revisions`, `get/list_routing_revisions`: exact version reads
  and ascending version lists. Lists accept an optional cap (100 default, 500
  maximum), following Inventory's bounded-read convention rather than Bilimbi's
  unbounded lists. All lists include retired records where applicable.
- `select_revisions`: exact formula/routing versions, independently numbered.
  Routing inputs/outputs must be subsets of their respective formula item sets,
  and some operation must output the product item. It returns all three records.

`process_config` accepts only `process_family` (string/null), `tolerances`,
`output_roles` and `material_hold_rules` (object/null). Nested values remain
company data. Quantities become decimal strings. Resource properties use the
same definitions, normalization and validation as material properties.

## Inventory and history

Publication checks materials and units through Inventory on the **same
transaction** and resources through this module. Retired references refuse new
publications. Bilimbi deliberately accepts `MaterialNotFound` (an item need not
be stocked); it does not check conversions or unit compatibility. Selection
does not revalidate retirement: historical revisions remain selectable.

Inventory reads retain their own capability checks: product creation additionally
needs `commerce.inventory.item.view`; publication needs
`factory.inventory.configuration.view`. The company write lock serializes
publication with Inventory retirement and company archival. A product lock and
unique constraint serialize revision numbering. Failed writes consume no version.

All five tables have tenant/company ownership. Sibling Inventory IDs are opaque
facade-validated references, not SQL dependencies on its tables. There is no
cross-container dependency. Revisions have no edit/delete API, and PostgreSQL
also refuses UPDATE, DELETE and TRUNCATE (stronger than the reference's
facade-only immutability). Every saved row is audited with complete old/new data.
There are no production seeds, agent operations, CSS or JavaScript.

## Runtime proof

Run the mounted Platform `scripts/domain-ci.sh` on an empty PostgreSQL 18
database. Then on a separate freshly migrated disposable database run the
container walkthrough's `product-definition` binary (root README). It uses
real sessions and the registered facades, including concurrent publication,
capability and company refusals, retirement, exact selection and SQL immutability.
