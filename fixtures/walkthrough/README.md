# Factory browser sweep

`factory-pages` hosts all twelve Factory routes with the same disposable manager and viewer accounts. It adds no domain records and is never a production dependency or seed.

From a real Platform mount, migrate a fresh disposable PostgreSQL 18 database, then run:

```sh
cp ../../../../../Cargo.lock .
BILIBI_FACTORY_FIXTURE=disposable \
  CARGO_TARGET_DIR=/path/to/platform/target \
  DATABASE_URL=postgres://…/disposable_factory \
  APP_ORIGIN=http://localhost:35605 BIND_ADDR=127.0.0.1:35605 \
  cargo run --bin factory-pages
```

Sign in at the exact `APP_ORIGIN` as `manager@example.test` or `viewer@example.test`, using the Platform walkthrough password. The manager has Factory configuration, capture and supervisor permissions; the viewer can read the twelve routes and has no Factory writes. Both belong to Company 001. Existing direct grants still take precedence, including denies installed by other walkthroughs.

Capture the empty routes before adding records: `materials`, `material-types`, `units`, `conversions`, `item-settings`, `resource-types`, `resources`, `definitions`, `floor`, `wastage-reasons`, `labour-roles`, and `measurement-types`, all under `/factory/`. Use both themes at 1440 × 1000 for the R1 reference, then a 768 × 1024 touch viewport for floor interactions. Keep screenshots, snapshots and comparisons outside the repository.

For populated floor flows, use a separate freshly migrated database. Run `production-execution`, `shop-floor`, `floor-pages`, then `labour-pages` once each with `BILIBI_FACTORY_FIXTURE=disposable` and the same `DATABASE_URL` and target directory. Finally run `factory-pages` against that database. Those walkthroughs exercise the facades and registered page adapters; the browser proves the controls and live transport. Use the order/run IDs printed by the walkthrough, rather than assuming sequence values. The root README and each module README describe their detailed contracts.

Check order/run selection, Back and reload, wastage recording and correction, all measurement value types and range flags, own and supervisor clocks, finished intervals and corrections, and FlexTable column settings/compact mode. With the viewer, check withheld controls and permission messages. Run the Platform `scripts/domain-ci.sh` separately against an empty database. No unit-test runner is involved.
