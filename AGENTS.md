# bilibi-dom-factory Agent Guide

This repository is the Factory Domain for Bilibi. It is not a standalone Cargo
project: it builds only when mounted in a Bilibi checkout at
`apps/domains/factory` (see `README.md`). Read the Platform's root and
`apps/domains/AGENTS.md`, `apps/base/ui/AGENTS.md` and
`apps/base/ports/README.md` in that checkout before changing anything here; they
govern this code and are not copied here. Bilimbi's
[`b-dom-factory`](https://github.com/BelimbingApp/b-dom-factory) is the
behaviour reference: port IDs, labels, capability keys, flash texts and
confirmations verbatim.

## Architecture

- Make each module deep: one small facade over its own records (`Inventory` in
  `inventory/src/lib.rs`), with schema, SQL, validation and helpers private,
  not a `pub` row type, query function or helper for a caller to reach. A
  later page in the same crate uses the facade like any caller. `pub` is for
  what the facade's signatures need and nothing else.
- Keep this Domain free of every other Domain: never add a Cargo dependency, a
  descriptor edge, a port call or a SQL reference (a table or column name) to
  another Domain container, because the containers must mount and build alone.
  A cross-domain flow will use Base domain events (a Platform slice in
  progress); until then leave a clearly named `TODO(<seam>)` where it belongs
  and reach across nothing.
- Between modules of this Domain, depend only on a facade, declared in the
  module's descriptor and `Cargo.toml`, only where Bilimbi has the same edge
  (Product Definition → Inventory; Production Execution → Inventory and Product
  Definition), and with no cycle. Do not read another module's table or call its
  private helper, and do not add an edge Bilimbi lacks.

## Working here

- Mount a real copy (`git clone` into `apps/domains/factory`), never a symlink,
  and commit only inside this repository. Never commit the Platform's
  `apps/domains/Cargo.toml`, `apps/domains/src/lib.rs` or `Cargo.lock`.
- The container root holds `bilibi.container.toml`, docs, CI and `fixtures/`
  only: no `Cargo.toml`, `[workspace]` or `Cargo.lock`. A module is an immediate
  child folder with its own `Cargo.toml` (crate `bilibi-factory-<module>`,
  inheriting the workspace), `README.md` (its owner contract), `migrations/` and
  `src/`; add `templates/` and `askama.toml` (`dirs = ["templates",
  "../../../base/ui/templates"]`) with the module's first page.
- Module IDs are `factory/<module>`. Name migrations
  `factory-<module>-NNN-<slug>` (underscores become hyphens), written as the end
  state of Bilimbi's schema, not its incremental history; never edit an applied
  file, add the next number. Every table carries `tenant_id` and `company_id`
  with a composite foreign key to `companies(tenant_id, id)`.
- Domain code uses only meta-terms: material, material type, resource, resource
  type, property, operation, output role, process family. A specific industry,
  material, product, line, status or currency name is a company's configuration
  data (its types, their property definitions, its item settings); it never
  appears in identifiers, schema constants, inclusion lists or defaults, and a
  fixture carries one only as data. Read item statuses and the default currency
  through `Inventory::item_settings`, never from a constant.
- For a property a company defines on its types, use `PropertyDefinition`
  and Inventory's `normalize_property_definitions`/`validate_property_values`
  facade (implemented in `inventory/src/property.rs`) and store the
  validated values on the typed row, as materials do; do not add a typed
  column or a second validator for one property, because the definitions are
  the company's and one mechanism keeps them behaving alike.
- Read Inventory inside a sibling's mutation through `get_item_on`,
  `get_material_on` and `get_unit_on`, not a second pool connection: this sees
  pending data, retains authorization and avoids pool exhaustion under locks.
- Do not hard-code what an operator could set (limits, scopes, codes,
  vocabularies): declare a Base setting with an editor, as `contributions.rs`
  does for `factory.inventory.item_statuses`.
- Write through `MutationBoundary`, not a bare transaction: `audited!` in
  `inventory/src/lib.rs` begins the transaction, the module's write returns every
  affected row as a `Change` (an empty `{}` for `old` on a create, or the audit
  event reads "updated"), and only a saved outcome commits. Check the capability
  in the facade on every write-shaped path (`access::writable`), and decide a
  refusal (a taken code, a retired record) before `mutate`, which treats a write
  with no `Change` as invalid.
- Use `bilibi_company::organisation::{authorize_target, require_live,
  require_writable}` for the company, not `companies` SQL of your own.
- Keep native ledger arithmetic in PostgreSQL, not `rust_decimal`: its
  `numeric(36,12)` quantities exceed Decimal's precision. Use the Inventory
  posting facade, not entry inserts, to preserve locking and retry semantics
  (contract: `inventory/README.md`, "Material Transaction ledger").
- Post production only through Inventory's four production postings, passing
  your module's private `static PostingAuthority` contributed to
  `POSTING_AUTHORITIES` (`inventory/src/authority.rs`); never carry production
  context on an ordinary posting or share the static, because Inventory
  accepts only the contributed static's address and boot refuses a declaration
  outside the `factory` container.
  A record that must commit with a posting uses the `_on` variants on your own
  `MutationTransaction` (`production_execution/src/completion.rs`), not a
  second connection or a post followed by an insert, so neither survives the
  other's failure.
- Create a lot or unit on the receipt or output line's `identity` and cite its
  `identity_id` later; read dimensions, positions and traces through the
  genealogy facade, and a mixed-unit transform's agreement through
  `get_transaction_balance`, never by converting quantities yourself, because
  posted observations and conversion versions are the record.
- A module's pages are a private `src/web` behind one `PageService`, with routes, menu items and its `PageWrites` (Bilibi `apps/base/agent_api/README.md`; the host refuses an undeclared live-event write at run time, not at build) in its `contributions.rs`; a live event that writes re-checks its capability itself (`web/shared.rs` `can`) before calling the facade, which checks once more. Do not trust the route guard alone, because a grant can be revoked while the page is open.
- In facade code collect any iterator into an owned `Vec`/`HashSet` before the first `.await` (as `product_definition/src/revisions.rs` does). A borrowed iterator held across an await makes the facade's future unprovably `Send`, and a page cannot await it: rustc reports "implementation of `Send` is not general enough" at the page, far from the cause.
- Expose an Inventory write to agents only as a reviewed safe write in
  `inventory/src/agent_operations.rs`, calling the facade's own code
  (`ledger::post`, `locations::create`) inside the plan's `MutationTransaction`
  and previewing it in a rolled-back transaction. Do not write a second
  validator, direct write or production posting there: the preview must refuse
  exactly what the commit will, and an agent holds no `PostingAuthority`.
- Name a page form's submit event for its action (`save_unit`), never `save`,
  because `live.js` reserves that name and nests its values under `edit`; a
  retirement waits on `ConfirmDialog`, never `data-confirm`, which the
  Platform does not run.
- Lists are `FlexTable` with `ListColumns`, and a page composes Platform
  components only (`bilibi_ui`); a Domain owns no CSS or JavaScript, so a class
  the Platform stylesheet lacks is a Platform change.
- Prove behaviour by running it, not with unit tests: `scripts/domain-ci.sh`
  from the Platform root with the container mounted and `DATABASE_URL` on an
  empty PostgreSQL 18 database, and `fixtures/walkthrough` (README, "Walkthrough
  fixture") for the facade. A proof creates its own data in a disposable
  database; the production seeds carry none.
- CI is `.github/workflows/ci.yml`, the Platform's `docs/domain-ci/ci.yml`
  unchanged but for `CONTAINER_ID`; the Platform revision it builds against is
  the single line in `.github/bilibi-revision`. Do not hand-roll another gate
  list.

## Maintaining this file

Keep this file for knowledge useful to almost every future agent session in this project.
Do not repeat what the codebase already shows; point to the authoritative file or command instead.
Prefer rewriting or pruning existing entries over appending new ones.
When updating this file, preserve this bar for all agents and keep entries concise.
