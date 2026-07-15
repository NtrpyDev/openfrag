#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use openfrag_import::ParsedEvent;
use serde_json::Value;
use std::collections::BTreeMap;

const MAX_INPUT_BYTES: usize = 64 * 1024;

#[derive(Arbitrary, Debug)]
struct NormalizerInput {
    name: String,
    tick: i32,
    ingestion_ordinal: u64,
    fields: Vec<(String, FieldValue)>,
}

#[derive(Arbitrary, Debug)]
enum FieldValue {
    Null,
    Bool(bool),
    Integer(i64),
    Text(String),
    Bytes(Vec<u8>),
}

fuzz_target!(|input: NormalizerInput| {
    let fields = input
        .fields
        .into_iter()
        .map(|(key, value)| {
            let value = match value {
                FieldValue::Null => Value::Null,
                FieldValue::Bool(value) => Value::Bool(value),
                FieldValue::Integer(value) => Value::from(value),
                FieldValue::Text(value) => Value::String(value),
                FieldValue::Bytes(value) => {
                    Value::Array(value.into_iter().map(Value::from).collect())
                }
            };
            (key, value)
        })
        .collect::<BTreeMap<_, _>>();
    let encoded_len = serde_json::to_vec(&fields).map_or(usize::MAX, |bytes| bytes.len());
    if encoded_len <= MAX_INPUT_BYTES {
        let _ = ParsedEvent::from_raw(&input.name, input.tick, input.ingestion_ordinal, &fields);
    }
});
