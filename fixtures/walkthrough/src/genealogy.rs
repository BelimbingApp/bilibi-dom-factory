//! XI3 live-facade walkthrough: lot and unit identities, transforms with a
//! variance per native unit, traces, draws, balances and the production
//! posting authority. Run once on a freshly migrated disposable database;
//! it checks each runtime outcome and exits nonzero on an unexpected one.
//!
//! The fixture declares itself a Factory module, `factory/walkthrough`, and
//! contributes the posting authority Production Execution will contribute
//! (as Bilimbi's Inventory tests declare `TestPostingAuthority`).
use bilibi_authz::{
    grants::Grants, CapabilityDefinition, RoleDefinition, VocabularyContribution, VOCABULARY,
};
use bilibi_factory_inventory::{
    Decimal, Dimension, DimensionName, DimensionProvenance, IdentityKind, Inventory, LengthUnit,
    MaterialTransaction, MaterialTyping, NewIdentity, NewItem, NewLocation, NewUnit, Observation,
    PostingAuthority, PostingRequest, StockLine, UnitVariance, Variance, POSTING_AUTHORITIES,
};
use bilibi_tenancy::Scope;
use bilibi_walkthrough_fixture::registry::{
    self, Catalog, Contribution, Layer, Module, Provenance,
};
use std::{collections::BTreeMap, fmt::Debug};

bilibi_walkthrough_fixture::walkthrough_vocabulary!();
bilibi_walkthrough_fixture::walkthrough_module_roles!();
bilibi_walkthrough_fixture::inventory::submit! { Contribution::new(VOCABULARY,"factory.xi3.roles",Provenance {module:"base/authz",source:file!(),line:line!()}, &VocabularyContribution {
    domains:&[],verbs:&[],capabilities:&[] as &[CapabilityDefinition],system_principals:&[],
    roles:&[
        RoleDefinition {code:"manager",name:"manager",description:"Walkthrough fixture role",grant_all:false,capabilities:&["factory.inventory.configuration.view","factory.inventory.configuration.manage","commerce.inventory.item.create","commerce.inventory.item.view"]},
        RoleDefinition {code:"viewer",name:"viewer",description:"Walkthrough fixture role",grant_all:false,capabilities:&["factory.inventory.configuration.view"]},
    ],
}) }

const MODULE: &str = "factory/walkthrough";
bilibi_walkthrough_fixture::inventory::submit! { Module { id: MODULE, layer: Layer::Domains, dependencies: &["factory/inventory"], migrations: &[], services: &[] } }
/// The registered authority. Private, as a declaring module keeps it.
static AUTHORITY: PostingAuthority = PostingAuthority::declare(MODULE);
bilibi_walkthrough_fixture::inventory::submit! { Contribution::new(POSTING_AUTHORITIES, MODULE, Provenance { module: MODULE, source: file!(), line: line!() }, &AUTHORITY) }
/// An equal value nobody contributed: not an authority.
static IMPOSTOR: PostingAuthority = PostingAuthority::declare(MODULE);

type Error = Box<dyn std::error::Error>;
fn refusal<T: Debug, E: std::error::Error + 'static>(
    label: &str,
    value: Result<T, E>,
    expected: &str,
) -> Result<(), Error> {
    match value {
        Err(error) => {
            // PostgreSQL's own message, when the refusal came from the database.
            let reason = (&error as &dyn std::error::Error)
                .downcast_ref::<tokio_postgres::Error>()
                .and_then(|error| error.as_db_error())
                .map_or_else(|| error.to_string(), |db| db.message().to_owned());
            if reason.contains(expected) {
                println!("{label}: refused ({reason})");
                return Ok(());
            }
            Err(format!("{label}: expected {expected}, got {reason}").into())
        }
        Ok(value) => Err(format!("{label}: expected {expected}, got {value:?}").into()),
    }
}
fn verify(label: &str, correct: bool) -> Result<(), Error> {
    if !correct {
        return Err(format!("{label}: unexpected runtime result").into());
    }
    println!("{label}: verified");
    Ok(())
}
async fn login(app: &bilibi_web::App, email: &str) -> Result<Scope, Error> {
    let login = app
        .user
        .login(email, bilibi_walkthrough_fixture::PASSWORD)
        .await?;
    Ok(app.security.guard(&login.token).await?)
}
fn decimal(value: &str) -> Decimal {
    value.parse().expect("decimal")
}
fn line(item: i64, location: i64, quantity: &str) -> StockLine {
    StockLine {
        item_id: item,
        location_id: Some(location),
        quantity: decimal(quantity),
        ..Default::default()
    }
}
fn lot(code: &str) -> Option<NewIdentity> {
    Some(NewIdentity {
        kind: IdentityKind::Lot,
        code: code.into(),
        dimensions: BTreeMap::new(),
    })
}
fn request(id: &str, lines: Vec<StockLine>) -> PostingRequest {
    PostingRequest {
        request_id: id.into(),
        evidence: "walkthrough source evidence".into(),
        lines,
        ..Default::default()
    }
}
fn transform(id: &str, inputs: Vec<StockLine>, outputs: Vec<StockLine>) -> PostingRequest {
    PostingRequest {
        request_id: id.into(),
        evidence: "walkthrough run record".into(),
        inputs,
        outputs,
        ..Default::default()
    }
}
fn shared(evidence: &str) -> Option<Variance> {
    Some(Variance::Shared {
        evidence: evidence.into(),
        reconciliation_basis: "observed in and out".into(),
    })
}
/// The identity a transaction's `index`th stock entry created or moved.
fn identity(transaction: &MaterialTransaction, index: usize) -> i64 {
    transaction
        .entries
        .iter()
        .filter(|entry| entry.role == "stock")
        .nth(index)
        .and_then(|entry| entry.identity_id)
        .expect("identified entry")
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    if std::env::var("BILIBI_FACTORY_FIXTURE").as_deref() != Ok("disposable") {
        return Err("set BILIBI_FACTORY_FIXTURE=disposable".into());
    }
    let db = bilibi_database::pool(&std::env::var("DATABASE_URL")?)?;
    bilibi_walkthrough_fixture::install(&db).await?;
    let app = bilibi_web::App::new(db.clone(), "http://localhost:35591")?;
    app.services
        .get::<Grants>("base/authz.grants")?
        .reconcile()
        .await?;
    let inventory = app
        .services
        .get::<Inventory>(bilibi_factory_inventory::SERVICE)?;
    let manager = login(&app, "manager@example.test").await?;
    let viewer = login(&app, "viewer@example.test").await?;
    let staff = login(&app, "staff@example.test").await?;
    let other = login(&app, "manager@other.example.test").await?;
    let client = db.get().await?;
    if client
        .query_one("SELECT count(*) FROM factory_inventory_transactions", &[])
        .await?
        .get::<_, i64>(0)
        != 0
    {
        return Err("use a fresh disposable database".into());
    }

    println!("== posting authority registry");
    verify(
        "the contributed static is registered",
        inventory.posting_authority_registered(&AUTHORITY),
    )?;
    verify(
        "an equal value built elsewhere is not",
        !inventory.posting_authority_registered(&IMPOSTOR),
    )?;
    // Boot validation: the catalog Inventory's consumer accepts, rebuilt
    // with one more declaration from outside the Factory container.
    let modules = registry::discover()?;
    let consumers: Vec<_> = bilibi_walkthrough_fixture::inventory::iter::<registry::Consumer>
        .into_iter()
        .collect();
    let mut contributions: Vec<&Contribution> =
        bilibi_walkthrough_fixture::inventory::iter::<Contribution>
            .into_iter()
            .collect();
    static FOREIGN: PostingAuthority = PostingAuthority::declare("core/company");
    static FOREIGN_CONTRIBUTION: Contribution = Contribution::new(
        POSTING_AUTHORITIES,
        "core/company",
        Provenance {
            module: "core/company",
            source: file!(),
            line: line!(),
        },
        &FOREIGN,
    );
    contributions.push(&FOREIGN_CONTRIBUTION);
    refusal(
        "a module outside the factory container declares an authority",
        Catalog::build(&modules, consumers.clone(), contributions).map(|_| ()),
        "only a Domain module of Inventory's own container",
    )?;
    static MISNAMED: PostingAuthority = PostingAuthority::declare("factory/inventory");
    static MISNAMED_CONTRIBUTION: Contribution = Contribution::new(
        POSTING_AUTHORITIES,
        "factory/inventory",
        Provenance {
            module: MODULE,
            source: file!(),
            line: line!(),
        },
        &MISNAMED,
    );
    let mut contributions: Vec<&Contribution> =
        bilibi_walkthrough_fixture::inventory::iter::<Contribution>
            .into_iter()
            .collect();
    contributions.push(&MISNAMED_CONTRIBUTION);
    refusal(
        "a factory module declares another module",
        Catalog::build(&modules, consumers, contributions).map(|_| ()),
        "only a Domain module of Inventory's own container",
    )?;

    println!("== catalog");
    let mut units = BTreeMap::new();
    for code in ["kg", "m2", "each"] {
        let unit = inventory
            .create_unit(
                &manager,
                1,
                &NewUnit {
                    code: code.into(),
                    name: code.into(),
                },
            )
            .await?;
        units.insert(code, unit.id);
    }
    let mut locations = Vec::new();
    for code in ["STORE", "FLOOR"] {
        let location = inventory
            .create_location(
                &manager,
                1,
                &NewLocation {
                    code: code.into(),
                    name: code.into(),
                },
            )
            .await?;
        locations.push(location.id);
    }
    let (store, floor) = (locations[0], locations[1]);
    let mut items = BTreeMap::new();
    for (sku, unit) in [
        ("XI3-SOURCE", "kg"),
        ("XI3-PART", "kg"),
        ("XI3-OFFCUT", "kg"),
        ("XI3-PIECE", "kg"),
        ("XI3-BASE", "m2"),
        ("XI3-ADDED", "kg"),
        ("XI3-COMBINED", "m2"),
    ] {
        let material = inventory
            .create_material(
                &manager,
                1,
                &NewItem {
                    sku: sku.into(),
                    title: sku.into(),
                    status: Some("active".into()),
                    currency_code: Some("MYR".into()),
                    ..Default::default()
                },
                units[unit],
                &MaterialTyping::default(),
            )
            .await?;
        items.insert(sku, material.item_id);
    }
    let (source, part, offcut, piece) = (
        items["XI3-SOURCE"],
        items["XI3-PART"],
        items["XI3-OFFCUT"],
        items["XI3-PIECE"],
    );

    println!("== lot and unit identities");
    let receipt = inventory
        .record_receipt(
            &manager,
            1,
            &request(
                "receipt-lot",
                vec![StockLine {
                    identity: lot("LOT-1"),
                    ..line(source, store, "100")
                }],
            ),
        )
        .await?;
    let lot_1 = identity(&receipt, 0);
    let read = inventory.get_identity(&viewer, 1, lot_1).await?;
    verify(
        "receipt creates the lot with its source transaction",
        read.kind == IdentityKind::Lot
            && read.code == "LOT-1"
            && read.item_id == source
            && read.source_transaction_id == receipt.id
            && read.dimensions.is_empty(),
    )?;
    refusal(
        "the same code again for the material",
        inventory
            .record_receipt(
                &manager,
                1,
                &request(
                    "receipt-lot-again",
                    vec![StockLine {
                        identity: lot("LOT-1"),
                        ..line(source, store, "1")
                    }],
                ),
            )
            .await,
        "Code has already been taken",
    )?;
    refusal(
        "a receipt citing an identity",
        inventory
            .record_receipt(
                &manager,
                1,
                &request(
                    "receipt-cites",
                    vec![StockLine {
                        identity_id: Some(lot_1),
                        ..line(source, store, "1")
                    }],
                ),
            )
            .await,
        "Invalid identity",
    )?;
    let width = Dimension {
        value: decimal("1200"),
        unit: LengthUnit::Mm,
        provenance: DimensionProvenance::Measured,
    };
    refusal(
        "dimensions on a lot",
        inventory
            .record_receipt(
                &manager,
                1,
                &request(
                    "receipt-lot-dimensions",
                    vec![StockLine {
                        identity: Some(NewIdentity {
                            kind: IdentityKind::Lot,
                            code: "LOT-D".into(),
                            dimensions: BTreeMap::from([(DimensionName::Width, width)]),
                        }),
                        ..line(source, store, "1")
                    }],
                ),
            )
            .await,
        "identity needs kind (lot or unit)",
    )?;
    inventory
        .record_receipt(
            &manager,
            1,
            &request("receipt-pool", vec![line(source, store, "20")]),
        )
        .await?;
    let transfer = inventory
        .record_transfer(
            &manager,
            1,
            &request(
                "transfer-lot",
                vec![StockLine {
                    location_id: None,
                    from_location_id: Some(store),
                    to_location_id: Some(floor),
                    identity_id: Some(lot_1),
                    ..line(source, store, "40")
                }],
            ),
        )
        .await?;
    verify(
        "a transfer moves the identity at both locations",
        transfer
            .entries
            .iter()
            .all(|entry| entry.identity_id == Some(lot_1)),
    )?;
    let positions = inventory.get_identity_positions(&viewer, 1, lot_1).await?;
    verify(
        "identity positions by location",
        positions
            .iter()
            .map(|p| (p.location.id, p.quantity.as_str(), p.unit.code.as_str()))
            .collect::<Vec<_>>()
            == vec![
                (store, "60.000000000000", "kg"),
                (floor, "40.000000000000", "kg"),
            ],
    )?;
    refusal(
        "drawing more unidentified stock than the pool holds",
        inventory
            .record_consumption(
                &manager,
                1,
                &request("consume-pool", vec![line(source, store, "30")]),
            )
            .await,
        "Insufficient unidentified stock",
    )?;
    refusal(
        "drawing more of the lot than it holds there",
        inventory
            .record_consumption(
                &manager,
                1,
                &request(
                    "consume-lot",
                    vec![StockLine {
                        identity_id: Some(lot_1),
                        ..line(source, store, "70")
                    }],
                ),
            )
            .await,
        "Insufficient identity stock",
    )?;
    let consumed = inventory
        .record_consumption(
            &manager,
            1,
            &request(
                "consume-lot-ok",
                vec![StockLine {
                    identity_id: Some(lot_1),
                    ..line(source, store, "10")
                }],
            ),
        )
        .await?;

    println!("== production context needs an authority");
    let mut production = request("ordinary-production", vec![line(source, store, "1")]);
    production
        .context
        .insert("operation_execution".into(), "OPX-1".into());
    refusal(
        "ordinary consumption with production context",
        inventory.record_consumption(&manager, 1, &production).await,
        "Unregistered posting authority",
    )?;
    refusal(
        "output through an unregistered value",
        inventory
            .record_output(
                &manager,
                1,
                &request("output-impostor", vec![line(part, floor, "1")]),
                &IMPOSTOR,
            )
            .await,
        "Unregistered posting authority",
    )?;
    let production_consumption = inventory
        .record_production_consumption(&manager, 1, &production, &AUTHORITY)
        .await?;
    verify(
        "production consumption records its authority and context",
        production_consumption.posting_authority.as_deref() == Some(MODULE)
            && production_consumption.context.get("operation_execution")
                == Some(&"OPX-1".to_owned()),
    )?;
    let output = inventory
        .record_output(
            &manager,
            1,
            &request(
                "output-lot",
                vec![StockLine {
                    identity: lot("PART-LOT"),
                    ..line(part, floor, "5")
                }],
            ),
            &AUTHORITY,
        )
        .await?;
    verify(
        "output creates its lot, posted by the authority",
        output.kind == "output"
            && output.posting_authority.as_deref() == Some(MODULE)
            && output.entries.iter().any(|e| e.role == "boundary"),
    )?;

    println!("== transforms");
    let unit_with = |code: &str, value: &str| {
        Some(NewIdentity {
            kind: IdentityKind::Unit,
            code: code.into(),
            dimensions: BTreeMap::from([
                (DimensionName::Width, width),
                (
                    DimensionName::Length,
                    Dimension {
                        value: decimal(value),
                        unit: LengthUnit::M,
                        provenance: DimensionProvenance::Nominal,
                    },
                ),
            ]),
        })
    };
    let outputs = || {
        vec![
            StockLine {
                identity: unit_with("PART-1", "250"),
                ..line(part, floor, "30")
            },
            StockLine {
                identity: lot("OFFCUT-1"),
                output_role: Some("trim".into()),
                observation: Observation::Derived,
                ..line(offcut, floor, "8")
            },
        ]
    };
    let input = || {
        vec![StockLine {
            identity_id: Some(lot_1),
            ..line(source, floor, "40")
        }]
    };
    refusal(
        "ordinary transform has no entry point but the authority",
        inventory
            .record_transform(
                &manager,
                1,
                &transform("t-1", input(), outputs()),
                &IMPOSTOR,
            )
            .await,
        "Unregistered posting authority",
    )?;
    refusal(
        "inputs and outputs differ without evidence",
        inventory
            .record_transform(
                &manager,
                1,
                &transform("t-1", input(), outputs()),
                &AUTHORITY,
            )
            .await,
        "Variance required",
    )?;
    refusal(
        "an identified transform with an unidentified output",
        inventory
            .record_transform(
                &manager,
                1,
                &transform("t-1", input(), vec![line(part, floor, "40")]),
                &AUTHORITY,
            )
            .await,
        "Identity required",
    )?;
    refusal(
        "a transform output citing an identity",
        inventory
            .record_transform(
                &manager,
                1,
                &transform(
                    "t-1",
                    input(),
                    vec![StockLine {
                        identity_id: Some(lot_1),
                        ..line(source, floor, "40")
                    }],
                ),
                &AUTHORITY,
            )
            .await,
        "Invalid identity",
    )?;
    let first = PostingRequest {
        variance: shared("weighed loss on the run sheet"),
        ..transform("t-1", input(), outputs())
    };
    let run = inventory
        .record_transform(&manager, 1, &first, &AUTHORITY)
        .await?;
    let variance: Vec<_> = run
        .entries
        .iter()
        .filter(|entry| entry.role == "variance")
        .collect();
    verify(
        "a 2 kg variance entry and one link per input and output",
        variance.len() == 1
            && variance[0].native_quantity == "2.000000000000"
            && variance[0].reconciliation_basis.as_deref() == Some("observed in and out")
            && run.genealogy.len() == 2
            && run.posting_authority.as_deref() == Some(MODULE),
    )?;
    let replay = inventory
        .record_transform(&manager, 1, &first, &AUTHORITY)
        .await?;
    verify(
        "a retried transform returns the original",
        replay.id == run.id,
    )?;
    let part_1 = identity(&run, 1);
    let offcut_1 = identity(&run, 2);
    let part_identity = inventory.get_identity(&viewer, 1, part_1).await?;
    verify(
        "a unit keeps its typed dimensions",
        part_identity.kind == IdentityKind::Unit
            && part_identity.dimensions[&DimensionName::Width] == width
            && part_identity.dimensions[&DimensionName::Length].value == decimal("250"),
    )?;
    verify(
        "the trim output keeps its role",
        run.entries
            .iter()
            .any(|e| e.output_role.as_deref() == Some("trim")),
    )?;
    refusal(
        "variance evidence where nothing differs",
        inventory
            .record_transform(
                &manager,
                1,
                &PostingRequest {
                    variance: shared("unused"),
                    ..transform(
                        "t-2",
                        vec![StockLine {
                            identity_id: Some(part_1),
                            ..line(part, floor, "30")
                        }],
                        vec![StockLine {
                            identity: lot("PIECE-1"),
                            ..line(piece, floor, "30")
                        }],
                    )
                },
                &AUTHORITY,
            )
            .await,
        "No variance",
    )?;
    let second = inventory
        .record_transform(
            &manager,
            1,
            &transform(
                "t-2",
                vec![StockLine {
                    identity_id: Some(part_1),
                    ..line(part, floor, "30")
                }],
                vec![StockLine {
                    identity: lot("PIECE-1"),
                    ..line(piece, floor, "30")
                }],
            ),
            &AUTHORITY,
        )
        .await?;
    let piece_1 = identity(&second, 1);

    println!("== traces and draws");
    let backward = inventory.trace_backward(&viewer, 1, piece_1).await?;
    verify(
        "backward from the second output reaches the received lot",
        backward.root.id == piece_1
            && backward.identities.iter().map(|i| i.id).collect::<Vec<_>>()
                == vec![lot_1, part_1, piece_1]
            && backward.links.len() == 2
            && backward.receipts.iter().map(|t| t.id).collect::<Vec<_>>() == vec![receipt.id],
    )?;
    let forward = inventory.trace_forward(&viewer, 1, lot_1).await?;
    verify(
        "forward from the lot reaches every descendant",
        forward.identities.iter().map(|i| i.id).collect::<Vec<_>>()
            == vec![lot_1, part_1, offcut_1, piece_1]
            && forward.links.len() == 3,
    )?;
    let draws = inventory.list_identity_draws(&viewer, 1, lot_1).await?;
    verify(
        "draws list the unlinked consumption and the transform, not the transfer",
        draws.iter().map(|t| t.id).collect::<Vec<_>>() == vec![consumed.id, run.id],
    )?;
    let positions = inventory.get_identity_positions(&viewer, 1, part_1).await?;
    verify("a fully drawn unit has no position", positions.is_empty())?;

    println!("== corrections and balances");
    refusal(
        "an ordinary correction of an authority's posting",
        inventory
            .record_correction(
                &manager,
                1,
                &PostingRequest {
                    reason: Some("recount".into()),
                    corrects_transaction_id: Some(run.id),
                    ..request(
                        "correct-run",
                        vec![StockLine {
                            identity_id: Some(offcut_1),
                            ..line(offcut, floor, "1")
                        }],
                    )
                },
            )
            .await,
        "Unregistered posting authority",
    )?;
    let correction = inventory
        .record_production_correction(
            &manager,
            1,
            &PostingRequest {
                reason: Some("recount".into()),
                corrects_transaction_id: Some(run.id),
                ..request(
                    "correct-run",
                    vec![StockLine {
                        identity_id: Some(offcut_1),
                        ..line(offcut, floor, "1")
                    }],
                )
            },
            &AUTHORITY,
        )
        .await?;
    let recorrection = inventory
        .record_production_correction(
            &manager,
            1,
            &PostingRequest {
                reason: Some("recount reversed".into()),
                corrects_transaction_id: Some(correction.id),
                ..request(
                    "correct-correction",
                    vec![StockLine {
                        identity_id: Some(offcut_1),
                        ..line(offcut, floor, "-0.5")
                    }],
                )
            },
            &AUTHORITY,
        )
        .await?;
    let corrections = inventory.list_corrections(&viewer, 1, run.id).await?;
    verify(
        "corrections follow corrections of corrections",
        corrections.iter().map(|t| t.id).collect::<Vec<_>>()
            == vec![correction.id, recorrection.id],
    )?;
    let balance = inventory
        .get_transaction_balance(&viewer, 1, run.id)
        .await?;
    verify(
        "a single-unit transform balances net of corrections",
        balance.per_unit.len() == 1
            && balance.per_unit[0].unit.code == "kg"
            && balance.per_unit[0].input == "40.000000000000"
            && balance.per_unit[0].output == "38.500000000000"
            && balance.per_unit[0].difference == "1.500000000000"
            && balance.per_unit[0].variance == "2.000000000000"
            && balance.cross_unit.is_empty()
            && balance.correction_transaction_ids == vec![correction.id, recorrection.id],
    )?;
    println!("{balance:?}");

    // A coating-shaped run: base material by area, an added material by
    // mass, the combined material by area. The base and combined materials
    // are recorded in kg, so a mass balance exists; no area balance does.
    let (base, added, combined) = (items["XI3-BASE"], items["XI3-ADDED"], items["XI3-COMBINED"]);
    let base_kg = inventory
        .define_conversion(&manager, 1, base, units["kg"], decimal("2"))
        .await?;
    let combined_kg = inventory
        .define_conversion(&manager, 1, combined, units["kg"], decimal("0.75"))
        .await?;
    for (id, item, quantity) in [("base-in", base, "100"), ("added-in", added, "80")] {
        inventory
            .record_receipt(&manager, 1, &request(id, vec![line(item, floor, quantity)]))
            .await?;
    }
    let in_kg = |item, quantity: &str| StockLine {
        unit_id: Some(units["kg"]),
        ..line(item, floor, quantity)
    };
    let mixed = |variance| PostingRequest {
        variance,
        ..transform(
            "mixed",
            vec![in_kg(base, "50"), line(added, floor, "80")],
            vec![in_kg(combined, "128")],
        )
    };
    refusal(
        "named evidence must name every differing unit",
        inventory
            .record_transform(
                &manager,
                1,
                &mixed(Some(Variance::Units(vec![UnitVariance {
                    unit_id: units["m2"],
                    evidence: "area trimmed".into(),
                    reconciliation_basis: "area in and out".into(),
                }]))),
                &AUTHORITY,
            )
            .await,
        "Variance required",
    )?;
    let coated = inventory
        .record_transform(
            &manager,
            1,
            &mixed(Some(Variance::Units(vec![
                UnitVariance {
                    unit_id: units["m2"],
                    evidence: "area trimmed".into(),
                    reconciliation_basis: "area in and out".into(),
                },
                UnitVariance {
                    unit_id: units["kg"],
                    evidence: "added material carried on the combined material".into(),
                    reconciliation_basis: "mass enters the combined material".into(),
                },
            ]))),
            &AUTHORITY,
        )
        .await?;
    let before = inventory
        .define_conversion(&manager, 1, base, units["kg"], decimal("3"))
        .await?;
    let balance = inventory
        .get_transaction_balance(&viewer, 1, coated.id)
        .await?;
    println!("{balance:?}");
    let groups: Vec<_> = balance
        .per_unit
        .iter()
        .map(|g| {
            (
                g.unit.code.as_str(),
                g.input.as_str(),
                g.output.as_str(),
                g.difference.as_str(),
                g.variance.as_str(),
            )
        })
        .collect();
    verify(
        "per-unit groups in first-seen order, each with its own variance",
        groups
            == vec![
                (
                    "m2",
                    "100.000000000000",
                    "96.000000000000",
                    "4.000000000000",
                    "4.000000000000",
                ),
                (
                    "kg",
                    "80.000000000000",
                    "0",
                    "80.000000000000",
                    "80.000000000000",
                ),
            ],
    )?;
    verify(
        "only a mass cross-unit balance, at the posted conversion versions",
        balance.cross_unit.len() == 1
            && balance.cross_unit[0].unit.code == "kg"
            && balance.cross_unit[0].input == "130.000000000000"
            && balance.cross_unit[0].output == "128.000000000000"
            && balance.cross_unit[0].difference == "2.000000000000"
            && balance.cross_unit[0]
                .conversions
                .iter()
                .map(|c| (c.item_id, c.conversion_id, c.version))
                .collect::<Vec<_>>()
                == vec![(base, base_kg.id, 1), (combined, combined_kg.id, 1)]
            && before.version == 2,
    )?;

    println!("== access and isolation");
    refusal(
        "staff reads an identity",
        inventory.get_identity(&staff, 1, lot_1).await,
        "",
    )?;
    refusal(
        "staff posts a transform",
        inventory
            .record_transform(
                &staff,
                1,
                &transform("staff", input(), outputs()),
                &AUTHORITY,
            )
            .await,
        "",
    )?;
    refusal(
        "another tenant's company",
        inventory.trace_forward(&other, 1, lot_1).await,
        "Company not found",
    )?;
    refusal(
        "an identity of another company",
        inventory.get_identity(&manager, 2, lot_1).await,
        "Identity not found",
    )?;

    println!("== database backstops");
    for (sql, expected) in [
        ("UPDATE factory_inventory_identities SET code='CHANGED'", "append-only"),
        ("DELETE FROM factory_inventory_identities", "append-only"),
        ("TRUNCATE factory_inventory_identities CASCADE", "append-only"),
        ("UPDATE factory_inventory_genealogy_links SET output_entry_id=input_entry_id", "append-only"),
        ("DELETE FROM factory_inventory_genealogy_links", "append-only"),
        ("TRUNCATE factory_inventory_genealogy_links", "append-only"),
        ("INSERT INTO factory_inventory_identities(tenant_id,company_id,material_id,source_transaction_id,kind,code,dimensions) SELECT tenant_id,company_id,material_id,source_transaction_id,'lot','LOT-SHAPE','{\"width\":{}}' FROM factory_inventory_identities LIMIT 1", "factory_inventory_identity_dimensions_shape"),
        ("INSERT INTO factory_inventory_transaction_entries(tenant_id,company_id,transaction_id,role,material_id,native_quantity,native_unit_id,identity_id) SELECT e.tenant_id,e.company_id,e.transaction_id,'boundary',e.material_id,1,e.native_unit_id,e.identity_id FROM factory_inventory_transaction_entries e WHERE e.identity_id IS NOT NULL LIMIT 1", "factory_inventory_transaction_entries_identity_stock"),
        ("INSERT INTO factory_inventory_genealogy_links(tenant_id,company_id,transaction_id,input_entry_id,output_entry_id) SELECT l.tenant_id,l.company_id,l.transaction_id,l.input_entry_id,(SELECT max(id) FROM factory_inventory_transaction_entries WHERE transaction_id<>l.transaction_id) FROM factory_inventory_genealogy_links l LIMIT 1", "factory_inventory_genealogy_links_output_entry_id_foreign"),
    ] {
        refusal(sql, client.execute(sql, &[]).await, expected)?;
    }
    // An entry naming an identity of another material.
    let sql = "INSERT INTO factory_inventory_transaction_entries(tenant_id,company_id,transaction_id,role,material_id,location_id,native_quantity,native_unit_id,recorded_quantity,recorded_unit_id,observation,identity_id) SELECT e.tenant_id,e.company_id,e.transaction_id,'stock',e.material_id,e.location_id,1,e.native_unit_id,1,e.native_unit_id,'measured',(SELECT id FROM factory_inventory_identities WHERE material_id<>e.material_id LIMIT 1) FROM factory_inventory_transaction_entries e WHERE e.role='stock' AND e.identity_id IS NOT NULL LIMIT 1";
    refusal(
        "an entry naming another material's identity",
        client.execute(sql, &[]).await,
        "factory_inventory_entries_identity_foreign",
    )?;
    let audited = client.query_one("SELECT (SELECT count(*) FROM factory_inventory_identities),(SELECT count(*) FROM base_audit_mutations WHERE auditable_type='factory_inventory_identities'),(SELECT count(*) FROM factory_inventory_genealogy_links),(SELECT count(*) FROM base_audit_mutations WHERE auditable_type='factory_inventory_genealogy_links')", &[]).await?;
    verify(
        "every identity and link is audited once",
        audited.get::<_, i64>(0) == audited.get::<_, i64>(1)
            && audited.get::<_, i64>(2) == audited.get::<_, i64>(3)
            && audited.get::<_, i64>(2) == 5,
    )?;
    println!("XI3 walkthrough complete");
    Ok(())
}
