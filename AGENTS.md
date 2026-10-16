# Factory domain rules

- Mount this repository as a real checkout at `apps/domains/factory`, then run `python3 scripts/discover-modules.py`; do not add a root Cargo workspace or commit Platform composition files.
- Keep Bilimbi module IDs, capability keys, labels and setting keys verbatim; they are persisted authorization and configuration vocabulary.
- Use Platform-owned UI and FlexTable for every list; this domain owns no CSS or JavaScript.
- Route every write through `MutationBoundary`, check capability at the event and facade boundary, and never expose an unguarded database write.
- Name migrations `factory-<module>-NNN-<slug>` and preserve applied migration files.
