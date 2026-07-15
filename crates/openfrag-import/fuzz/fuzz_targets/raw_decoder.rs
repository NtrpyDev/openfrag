#![no_main]

use libfuzzer_sys::fuzz_target;
use openfrag_import::parse_pinned_demoparser_bytes;

const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;

fuzz_target!(|data: &[u8]| {
    if data.len() <= MAX_INPUT_BYTES {
        let _ = parse_pinned_demoparser_bytes(data);
    }
});
