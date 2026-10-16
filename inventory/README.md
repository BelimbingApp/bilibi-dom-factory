# Factory Inventory

Module ID: `factory/inventory`. This first slice establishes the durable vocabulary, settings and Unit/Location records. Unit codes are company-unique (maximum 32 characters); location codes are company-unique and upper-case (maximum 64 characters). Neither record is a material posting; later slices own material catalog and ledger behavior.

Item settings are `factory.inventory.item_statuses` and `factory.inventory.default_currency_code`, available at tenant and company scope with no module default. The initial schema is deliberately limited to the Unit and Location catalog, not the later material/catalog/ledger tables.
