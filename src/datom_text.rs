//! The Datom text form of a contract value.
//!
//! The component CLIs take one inline Datom value and print one Datom value.
//! The descent — delineate, conceive, incorporate — is `actualize`; the
//! ascent — datomize, protosize, print — is `textualize`, and cannot fault.

use datom_codec::{Actualizing, Budget, Compositional, Datomizable, Potential};
use protos::{Protosizable, ReaderBudget, Textualizable};

use crate::{Error, Result};

/// How much text, and how deep a structure, one CLI argument may carry.
const ARGUMENT_BUDGET: i64 = 1 << 20;
const MAXIMUM_DEPTH: i64 = 1024;

fn argument_budget() -> Budget {
    Budget {
        remaining: ARGUMENT_BUDGET,
        reader: ReaderBudget {
            remaining: ARGUMENT_BUDGET as usize,
        },
        depth: 0,
        maximum_depth: MAXIMUM_DEPTH,
    }
}

/// Read one contract value from its Datom text.
pub fn actualize<Value: Compositional>(text: &str) -> Result<Value> {
    Potential::<Value>::from(text)
        .actualize(&mut argument_budget())
        .map_err(|error| Error::Datom {
            detail: format!("{error:?}"),
        })
}

/// Project one contract value into its Datom text.
pub fn textualize<Value>(value: &Value) -> String
where
    Value: Datomizable,
    Value::Output: Protosizable,
    <Value::Output as Protosizable>::Output: Textualizable,
{
    value.datomize(Vec::new()).protosize().textualize()
}
