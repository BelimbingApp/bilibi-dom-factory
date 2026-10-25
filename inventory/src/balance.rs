//! Bilimbi Inventory.Balance: one transaction's material balance, net of its
//! corrections. Every sum runs in PostgreSQL over the posted entries, so the
//! figures are exact decimal text and nothing is converted at read time.
use crate::{access, ledger, Decimal, Error, Unit};
use bilibi_tenancy::Scope;
use serde::Deserialize;
use tokio_postgres::GenericClient;

/// The material balance of one transaction net of its corrections.
///
/// `per_unit` has one group per native unit among the stock entries, in the
/// order the units first appear: the observed `input` drawn from stock and
/// `output` put into it, `difference` (input less output), and the
/// `variance` the transaction recorded in that unit. Nothing is converted
/// between groups.
///
/// `cross_unit` is measurement agreement across units, present only when the
/// stock entries span more than one native unit, and only in a unit every
/// line was posted in: natively at its native quantity, or recorded in that
/// unit at its recorded quantity, naming the conversion version it was
/// posted through. A later conversion version never restates it. No entry
/// records a cross-unit difference.
#[derive(Clone, Debug)]
pub struct Balance {
    pub transaction_id: i64,
    pub per_unit: Vec<UnitBalance>,
    pub cross_unit: Vec<CrossUnitBalance>,
    pub correction_transaction_ids: Vec<i64>,
}

/// Quantities are exact decimal text in the group's native unit.
#[derive(Clone, Debug)]
pub struct UnitBalance {
    pub unit: Unit,
    pub input: String,
    pub output: String,
    pub difference: String,
    pub variance: String,
}

#[derive(Clone, Debug)]
pub struct CrossUnitBalance {
    pub unit: Unit,
    pub input: String,
    pub output: String,
    pub difference: String,
    pub conversions: Vec<BalanceConversion>,
}

/// The conversion a recorded line was posted through.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct BalanceConversion {
    pub item_id: i64,
    pub conversion_id: i64,
    pub version: i32,
    pub factor: Decimal,
}

/// The transaction's stock entries and its corrections' (`$1` tenant, `$2`
/// company, `$3` transaction). A correction entry nets into the side of the
/// entry it adjusts, the first of the transaction's for the same material and
/// identity, else its own sign's side.
const SIDED: &str = "WITH RECURSIVE corrections(id) AS (
  SELECT id FROM factory_inventory_transactions WHERE tenant_id=$1 AND company_id=$2 AND kind='correction' AND corrects_transaction_id=$3
  UNION SELECT t.id FROM factory_inventory_transactions t JOIN corrections c ON t.corrects_transaction_id=c.id WHERE t.tenant_id=$1 AND t.company_id=$2 AND t.kind='correction'),
own AS (SELECT * FROM factory_inventory_transaction_entries WHERE tenant_id=$1 AND company_id=$2 AND transaction_id=$3 AND role='stock'),
sided AS (
  SELECT ARRAY[0,e.transaction_id,e.id] AS ord, CASE WHEN e.native_quantity<0 THEN 'input' ELSE 'output' END AS side, e.material_id, e.native_unit_id, e.native_quantity, e.recorded_unit_id, e.recorded_quantity, e.conversion_id FROM own e
  UNION ALL
  SELECT ARRAY[1,e.transaction_id,e.id], COALESCE((SELECT CASE WHEN o.native_quantity<0 THEN 'input' ELSE 'output' END FROM own o WHERE o.material_id=e.material_id AND o.identity_id IS NOT DISTINCT FROM e.identity_id ORDER BY o.id LIMIT 1), CASE WHEN e.native_quantity<0 THEN 'input' ELSE 'output' END), e.material_id, e.native_unit_id, e.native_quantity, e.recorded_unit_id, e.recorded_quantity, e.conversion_id
  FROM factory_inventory_transaction_entries e WHERE e.tenant_id=$1 AND e.company_id=$2 AND e.role='stock' AND e.transaction_id IN (SELECT id FROM corrections)),
units AS (SELECT native_unit_id AS unit_id, min(ord) AS first FROM sided GROUP BY native_unit_id)";

const UNIT: &str = "u.id AS unit_id,u.code AS unit_code,u.name AS unit_name,u.retired_at IS NOT NULL AS unit_retired";

pub(crate) async fn read(
    client: &impl GenericClient,
    scope: &Scope,
    company: i64,
    id: i64,
) -> Result<Balance, Error> {
    access::readable(client, scope, company).await?;
    read_authorized(client, scope, company, id).await
}

pub(crate) async fn read_authorized(
    client: &impl GenericClient,
    scope: &Scope,
    company: i64,
    id: i64,
) -> Result<Balance, Error> {
    let tenant = scope.tenant();
    ledger::read(client, tenant, company, id).await?;
    let unit = |row: &tokio_postgres::Row| Unit {
        id: row.get("unit_id"),
        company_id: company,
        code: row.get("unit_code"),
        name: row.get("unit_name"),
        retired: row.get("unit_retired"),
    };
    let per_unit: Vec<UnitBalance> = client.query(&format!("{SIDED}
SELECT {UNIT}, x.input::text, x.output::text, (x.input-x.output)::text AS difference,
  (SELECT COALESCE(sum(v.native_quantity),0) FROM factory_inventory_transaction_entries v WHERE v.tenant_id=$1 AND v.company_id=$2 AND v.transaction_id=$3 AND v.role='variance' AND v.native_unit_id=x.unit_id)::text AS variance
FROM (SELECT n.unit_id, n.first, -COALESCE(sum(s.native_quantity) FILTER (WHERE s.side='input'),0) AS input, COALESCE(sum(s.native_quantity) FILTER (WHERE s.side='output'),0) AS output
      FROM units n JOIN sided s ON s.native_unit_id=n.unit_id GROUP BY n.unit_id, n.first) x
JOIN factory_inventory_units u ON u.company_id=$2 AND u.id=x.unit_id ORDER BY x.first"), &[&tenant, &company, &id]).await?.iter().map(|row| UnitBalance {
        unit: unit(row),
        input: row.get("input"),
        output: row.get("output"),
        difference: row.get("difference"),
        variance: row.get("variance"),
    }).collect();
    let mut cross_unit = Vec::new();
    if per_unit.len() > 1 {
        // A line counts in a unit only as it was posted: its native quantity
        // when native in that unit, or its recorded quantity when recorded in
        // it.
        for row in client.query(&format!("{SIDED},
candidates AS (SELECT n.unit_id, n.first FROM units n WHERE NOT EXISTS(SELECT 1 FROM sided s WHERE s.native_unit_id<>n.unit_id AND s.recorded_unit_id IS DISTINCT FROM n.unit_id)),
counted AS (SELECT c.unit_id, c.first, s.side, s.material_id, s.native_unit_id, s.conversion_id, CASE WHEN s.native_unit_id=c.unit_id THEN s.native_quantity WHEN s.native_quantity<0 THEN -s.recorded_quantity ELSE s.recorded_quantity END AS quantity FROM candidates c CROSS JOIN sided s)
SELECT {UNIT}, x.input::text, x.output::text, (x.input-x.output)::text AS difference,
  (SELECT COALESCE(jsonb_agg(to_jsonb(v) ORDER BY v.item_id, v.version), '[]'::jsonb) FROM (SELECT DISTINCT m.item_id, k.conversion_id, cv.version, cv.factor::text AS factor FROM counted k JOIN factory_inventory_materials m ON m.company_id=$2 AND m.id=k.material_id JOIN factory_inventory_unit_conversions cv ON cv.company_id=$2 AND cv.id=k.conversion_id WHERE k.unit_id=x.unit_id AND k.native_unit_id<>x.unit_id) v) AS conversions
FROM (SELECT unit_id, first, -COALESCE(sum(quantity) FILTER (WHERE side='input'),0) AS input, COALESCE(sum(quantity) FILTER (WHERE side='output'),0) AS output FROM counted GROUP BY unit_id, first) x
JOIN factory_inventory_units u ON u.company_id=$2 AND u.id=x.unit_id ORDER BY x.first"), &[&tenant, &company, &id]).await? {
            cross_unit.push(CrossUnitBalance {
                unit: unit(&row),
                input: row.get("input"),
                output: row.get("output"),
                difference: row.get("difference"),
                conversions: serde_json::from_value(row.get("conversions"))
                    .map_err(|_| bilibi_database::Error::Invalid)?,
            });
        }
    }
    let correction_transaction_ids = client
        .query(
            &format!("{SIDED} SELECT id FROM corrections ORDER BY id"),
            &[&tenant, &company, &id],
        )
        .await?
        .iter()
        .map(|row| row.get(0))
        .collect();
    Ok(Balance {
        transaction_id: id,
        per_unit,
        cross_unit,
        correction_transaction_ids,
    })
}
