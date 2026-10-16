# Bilibi Factory domain

This repository is the `factory` Domain container for Bilibi and is not built standalone. Mount it as a real nested Git checkout at `apps/domains/factory` in the pinned Bilibi Platform. Inventory's initial slice contributes the durable Factory vocabulary, settings, Units and Materials menu entries, and unit/location persistence. Further slices add the catalog, material ledger and production modules.

CI is the Platform's mounted-domain gate in `.github/workflows/ci.yml`. Inventory fixtures live under `fixtures/walkthrough`; production startup does not seed demo data.
