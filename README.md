# bilibi-dom-factory

Factory Domain for [Bilibi](https://github.com/BelimbingApp/bilibi): Inventory,
Product Definition and Production Execution. It is one
optional, independently versioned repository that a manufacturing adopter
mounts into a Bilibi checkout; it does not build on its own. It is the Rust
port of Bilimbi's [`b-dom-factory`](https://github.com/BelimbingApp/b-dom-factory),
with the same module IDs, capability keys, table names and wording.

| Module | Descriptor ID | Crate | State |
| --- | --- | --- | --- |
| [`inventory/`](inventory/README.md) | `factory/inventory` | `bilibi-factory-inventory` | units, stock locations and item settings (X0); item catalog (XI1); audited Material Transaction ledger, weighed receipts, transfers, consumption, corrections and stock reads (XI2); lot and unit genealogy, transforms, balances and the production posting authority (XI3); agent operations for locations, postings, stock, transaction and genealogy reads (H5); the five Inventory pages (XI4) |
| [`product_definition/`](product_definition/README.md) | `factory/product_definition` | `bilibi-factory-product-definition` | resource types, resources, products and immutable formula/routing revisions (XD1); Resource types, Resources and BOM formulas & routings pages (XD2) |
| [`production_execution/`](production_execution/README.md) | `factory/production_execution` | `bilibi-factory-production-execution` | production orders and live/import operation completion posted atomically through Inventory, with material hold rules and overrides (XP1); shop-floor capture and pages follow |

Factory declares no dependency on any other Domain container, and no Domain
depends on it; modules inside it depend on each other only through their
facades (see [`AGENTS.md`](AGENTS.md)).

## Mount

Clone this repository into a Bilibi checkout at `apps/domains/factory` (a real
checkout, never a symlink), then work from the Bilibi root:

```sh
git clone https://github.com/BelimbingApp/bilibi-dom-factory.git apps/domains/factory
python3 scripts/discover-modules.py
cargo build -p bilibi-web
```

Discovery reads [`bilibi.container.toml`](bilibi.container.toml); no list names
the container. Removing the folder removes Factory's code from the next build
and keeps its data. The mount, naming, composition and pinning rules are in the
Platform's `apps/domains/AGENTS.md`.

## CI

[`.github/workflows/ci.yml`](.github/workflows/ci.yml) is the Platform's
reference Domain workflow. It checks out Bilibi at the revision in
[`.github/bilibi-revision`](.github/bilibi-revision), mounts this repository
and runs `scripts/domain-ci.sh`: discovery, the committed-CSS check, `cargo
fmt`, `clippy -D warnings`, the locked release build, `migrate`,
`seed-production`, and a boot that loads `/`. Reproduce it locally with the
container mounted and `DATABASE_URL` on an empty PostgreSQL 18 database:

```sh
DATABASE_URL=postgres://… ./scripts/domain-ci.sh
```

Bump `.github/bilibi-revision` to build against a newer Platform.

## Walkthrough fixture

[`fixtures/walkthrough`](fixtures/walkthrough/src/main.rs) is not a module and
not product data: discovery skips `fixtures/`, and it is its own Cargo
workspace root so a mounted checkout can run it. On a disposable, freshly
migrated database it installs the Platform walkthrough fixture and then drives
the Inventory facade as real signed-in accounts, printing every outcome and
refusal. It asserts nothing; read the output.

```sh
cargo run -q -p bilibi-web -- migrate        # from the Bilibi root, DATABASE_URL set
cd apps/domains/factory/fixtures/walkthrough
cp ../../../../../Cargo.lock .               # same dependency versions as the Platform
BILIBI_FACTORY_FIXTURE=disposable cargo run
```

Set `CARGO_TARGET_DIR` to share build output with the Platform. The fixture
creates its own data; production installs carry none (`seed-production` seeds
nothing for Factory).

XI2's walkthrough includes concurrent withdrawals and retries, conversion
version preservation, weighed receipts, signed corrections, stock retirement,
permission and company isolation, and PostgreSQL's append-only and deferred
balance refusals. These facade-only operations have no pages in the reference;
run the walkthrough to exercise them rather than following the XI4 menu links.

For XI3, use a separate freshly migrated disposable database and run
`BILIBI_FACTORY_FIXTURE=disposable cargo run --bin genealogy`. It registers a
fixture Factory module, `factory/walkthrough`, as the posting authority (as
Bilimbi's tests declare `TestPostingAuthority`) and checks identities with
dimensions, transforms with per-unit variance, traces, draws, corrections,
per-unit and cross-unit balances, the authority registry's boot refusals,
access and PostgreSQL refusals; it exits nonzero on an unexpected result.

For XD2, the pages: on a separate freshly migrated disposable database run
`BILIBI_FACTORY_FIXTURE=disposable APP_ORIGIN=http://localhost:3020 BIND_ADDR=127.0.0.1:3020 cargo run --bin product-definition-pages`
from the same fixture folder. It installs the fixture accounts, one unit and two
items, and serves the composed app; sign in at `http://localhost:3020` (use
`localhost`, the origin the session cookie is issued for) as
`manager@example.test` (manages) or `viewer@example.test` (reads), password
`bilibi-dev`, and open Administration → Factory → Resources, BOM formulas &
routings, or `/factory/resource-types`.

For XD1, use a separate freshly migrated disposable database and run
`BILIBI_FACTORY_FIXTURE=disposable cargo run --bin product-definition` from
the same fixture folder. This binary checks its runtime outcomes and exits
nonzero on unexpected results. It covers typed properties, publication and
concurrent version allocation, exact selection, retirement, authorization,
tenant/company isolation, SQL immutability and audit records. No Product
Definition pages exist until XD2; the facade walkthrough is its runnable entry.

For H5, use a separate freshly migrated disposable database and run
`BILIBI_FACTORY_FIXTURE=disposable cargo run --bin agent-operations` from the
same fixture folder. It signs in as real accounts and drives Inventory's agent
operations through the registry and the reviewed safe-write plan service:
discovery by a manager and a viewer, location creation, receipts (a converted
unit, a lot, a weigh ticket), replays and request-id conflicts, transfers,
consumption refused for insufficient stock, a review outrun by a later
posting, a grant revoked between review and commit, a signed correction, the
transaction, balance and genealogy reads, and another tenant's refusal. It
checks each outcome and exits nonzero on an unexpected one; plan codes are
never printed. With `BILIBI_FACTORY_API=http://127.0.0.1:3012` set and the
composed app running against the same database (`BIND_ADDR=127.0.0.1:3012
APP_ORIGIN=http://localhost:3012 bilibi-web run`), it ends by approving a real
OAuth device grant and driving search, a refused direct write, a plan review,
its commit and a refused replay through `/api/v1`.

For XP1, use a separate freshly migrated disposable database and run
`BILIBI_FACTORY_FIXTURE=disposable cargo run --bin production-execution` from
the same fixture folder. It checks its runtime outcomes and exits nonzero on
unexpected ones: orders, live and imported completion posted through the
Inventory authority, idempotent and concurrent retries, atomic rollback, hold
rules and overrides, authorization, SQL immutability and audit. Production
Execution has no pages until XP3/XP4; the facade walkthrough is its runnable
entry.

## License

[MIT](LICENSE).
