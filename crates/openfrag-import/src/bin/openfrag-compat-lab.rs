use openfrag_import::{
    ParserExecutionMode, compatibility_hashes, normalized_event_schema,
    parse_pinned_demoparser_bytes, parse_pinned_demoparser_bytes_with_progress, query_plan_hash,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, env, fs, path::Path};

fn main() {
    let args = env::args().collect::<Vec<_>>();
    let result = match args.get(1).map(String::as_str) {
        Some("inspect") if args.len() == 3 => inspect_command(Path::new(&args[2])),
        Some("check-manifest") if args.len() == 3 => {
            check_manifest_command(Path::new(&args[2]))
        }
        Some("verify") if args.len() == 4 => {
            verify_command(Path::new(&args[2]), Path::new(&args[3]))
        }
        _ => Err(
            "usage: openfrag-compat-lab inspect <demo> | check-manifest <manifest> | verify <manifest> <corpus-dir>"
                .to_owned(),
        ),
    };
    if let Err(error) = result {
        eprintln!("compatibility lab failed: {error}");
        std::process::exit(1);
    }
}

fn inspect_command(path: &Path) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let report = inspect_bytes(&bytes)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn inspect_bytes(bytes: &[u8]) -> Result<Value, String> {
    let canonical = parse_pinned_demoparser_bytes(bytes).map_err(parse_error)?;
    let repeated = parse_pinned_demoparser_bytes(bytes).map_err(parse_error)?;
    let canonical_deterministic = canonical == repeated;
    let hashes = compatibility_hashes(&canonical);
    let parallel_first = parse_pinned_demoparser_bytes_with_progress(
        bytes,
        ParserExecutionMode::ExperimentalParallel,
        &mut |_| {},
    );
    let parallel = match parallel_first {
        Ok(first) => {
            let second = parse_pinned_demoparser_bytes_with_progress(
                bytes,
                ParserExecutionMode::ExperimentalParallel,
                &mut |_| {},
            )
            .map_err(parse_error)?;
            json!({
                "disposition": if first == canonical { "equivalent" } else { "diverges" },
                "deterministic": first == second,
                "normalized_evidence_sha256": compatibility_hashes(&first).normalized_evidence_sha256,
            })
        }
        Err(error) => json!({
            "disposition": "error",
            "category": error.diagnostic.category.code(),
        }),
    };
    Ok(json!({
        "sha256": format!("{:x}", Sha256::digest(bytes)),
        "bytes": bytes.len(),
        "query_plan_hash": canonical.identity.requested_schema_hash,
        "map": canonical.metadata.map,
        "cs2_build": canonical.metadata.patch_build,
        "demo_stamp": canonical.metadata.demo_stamp,
        "game_directory": canonical.metadata.game_directory,
        "tick_rate": canonical.metadata.tick_rate,
        "participants": canonical.participants.len(),
        "rounds": canonical.rounds.len(),
        "normalized_events": canonical.events.len(),
        "receipts": canonical.receipts.len(),
        "player_snapshots": canonical.player_snapshots.len(),
        "normalized_evidence_sha256": hashes.normalized_evidence_sha256,
        "receipt_hashes_sha256": hashes.receipt_hashes_sha256,
        "canonical_deterministic": canonical_deterministic,
        "parallel": parallel,
    }))
}

fn check_manifest_command(path: &Path) -> Result<(), String> {
    let manifest = read_json(path)?;
    check_manifest(&manifest)?;
    println!("compatibility manifest is structurally complete");
    Ok(())
}

fn check_manifest(manifest: &Value) -> Result<(), String> {
    if manifest["schema_version"].as_u64() != Some(2) {
        return Err("schema_version must be 2".into());
    }
    if manifest["query_plan_hash"].as_str() != Some(&query_plan_hash()) {
        return Err("manifest query plan does not match production".into());
    }
    let schema_bytes =
        serde_json::to_vec(&normalized_event_schema()).map_err(|error| error.to_string())?;
    let schema_hash = format!("{:x}", Sha256::digest(schema_bytes));
    if manifest["normalized_schema_sha256"].as_str() != Some(&schema_hash) {
        return Err("manifest normalized schema hash is stale".into());
    }
    let limits = &manifest["resource_limits"];
    for field in [
        "raw_fuzz_max_len_bytes",
        "normalizer_fuzz_max_len_bytes",
        "per_input_timeout_seconds",
        "rss_limit_mb",
    ] {
        if limits[field].as_u64().is_none_or(|value| value == 0) {
            return Err(format!("resource limit {field} must be positive"));
        }
    }
    let fixtures = manifest["fixtures"]
        .as_array()
        .ok_or_else(|| "fixtures must be an array".to_owned())?;
    let mut ids = BTreeSet::new();
    let mut shas = BTreeSet::new();
    let mut cohorts = BTreeSet::new();
    let mut edge_cases = BTreeSet::new();
    let mut synthetic_kinds = BTreeSet::new();
    for fixture in fixtures {
        let id = required_string(fixture, "id")?;
        if !ids.insert(id) {
            return Err("fixture IDs must be unique".into());
        }
        match required_string(fixture, "kind")? {
            "local_demo" => {
                let sha = required_string(fixture, "sha256")?;
                if sha.len() != 64 || !shas.insert(sha) {
                    return Err("local fixture SHA-256 values must be unique".into());
                }
                required_string(fixture, "map")?;
                required_string(fixture, "cs2_build")?;
                required_string(&fixture["provenance"], "source")?;
                if fixture["provenance"]["redistributable"].as_bool() != Some(false) {
                    return Err("local Demo bytes must remain non-redistributable".into());
                }
                cohorts.insert(required_string(fixture, "build_cohort")?);
                required_string(fixture, "mode")?;
                for edge in fixture["edge_cases"]
                    .as_array()
                    .ok_or_else(|| format!("fixture {id} edge_cases must be an array"))?
                {
                    edge_cases.insert(
                        edge.as_str()
                            .ok_or_else(|| format!("fixture {id} edge case must be a string"))?,
                    );
                }
                let expected = &fixture["expected"];
                for field in [
                    "participants",
                    "rounds",
                    "normalized_events",
                    "receipts",
                    "player_snapshots",
                ] {
                    if expected[field].as_u64().is_none() {
                        return Err(format!("fixture {id} expected {field} is missing"));
                    }
                }
                required_hash(expected, "normalized_evidence_sha256")?;
                required_hash(expected, "receipt_hashes_sha256")?;
                if fixture["threading"]["canonical_repeats"]
                    .as_u64()
                    .unwrap_or(0)
                    < 2
                {
                    return Err(format!("fixture {id} must repeat canonical parsing"));
                }
                required_string(&fixture["threading"], "experimental_parallel")?;
            }
            "synthetic_invalid" => {
                synthetic_kinds.insert(required_string(fixture, "generator")?);
                required_string(&fixture["expected"], "category")?;
            }
            other => return Err(format!("unsupported fixture kind {other}")),
        }
    }
    for required in ["current", "previous"] {
        if !cohorts.contains(required) {
            return Err(format!("missing {required} build cohort"));
        }
    }
    for required in ["same_tick_events"] {
        if !edge_cases.contains(required) {
            return Err(format!("missing edge-case coverage: {required}"));
        }
    }
    for required in ["truncated_frame", "unsupported_command", "short_header"] {
        if !synthetic_kinds.contains(required) {
            return Err(format!("missing synthetic invalid case: {required}"));
        }
    }
    let gaps = manifest["coverage_gaps"]
        .as_array()
        .ok_or_else(|| "coverage_gaps must be an array".to_owned())?;
    if gaps.is_empty() || gaps.iter().any(|gap| gap.as_str().is_none()) {
        return Err("coverage gaps must be explicit strings".into());
    }
    Ok(())
}

fn verify_command(manifest_path: &Path, corpus: &Path) -> Result<(), String> {
    let manifest = read_json(manifest_path)?;
    check_manifest(&manifest)?;
    let fixtures = manifest["fixtures"]
        .as_array()
        .ok_or_else(|| "fixtures must be an array".to_owned())?;
    let mut verified = 0_u64;
    for fixture in fixtures {
        match required_string(fixture, "kind")? {
            "local_demo" => {
                let sha = required_string(fixture, "sha256")?;
                let bytes = fs::read(corpus.join(format!("{sha}.dem")))
                    .map_err(|error| format!("fixture {sha}: {error}"))?;
                let observed = inspect_bytes(&bytes)?;
                verify_local_fixture(fixture, &observed)?;
            }
            "synthetic_invalid" => verify_synthetic_fixture(fixture)?,
            _ => unreachable!(),
        }
        verified += 1;
    }
    println!("verified {verified} compatibility fixtures");
    Ok(())
}

fn verify_local_fixture(expected: &Value, observed: &Value) -> Result<(), String> {
    let id = required_string(expected, "id")?;
    for field in ["sha256", "map", "cs2_build"] {
        if expected[field] != observed[field] {
            return Err(format!("fixture {id} changed {field}"));
        }
    }
    for field in [
        "participants",
        "rounds",
        "normalized_events",
        "receipts",
        "player_snapshots",
        "normalized_evidence_sha256",
        "receipt_hashes_sha256",
    ] {
        if expected["expected"][field] != observed[field] {
            return Err(format!("fixture {id} changed expected {field}"));
        }
    }
    if observed["canonical_deterministic"] != Value::Bool(true) {
        return Err(format!(
            "fixture {id} canonical output is not deterministic"
        ));
    }
    if expected["threading"]["experimental_parallel"] != observed["parallel"]["disposition"]
        || observed["parallel"]["deterministic"] == Value::Bool(false)
    {
        return Err(format!("fixture {id} parallel evidence changed"));
    }
    Ok(())
}

fn verify_synthetic_fixture(fixture: &Value) -> Result<(), String> {
    const MAGIC: &[u8] = b"PBDEMS2\0";
    let bytes = match required_string(fixture, "generator")? {
        "truncated_frame" => [MAGIC, &[0_u8; 8], &[1, 5, 10, 0, 0]].concat(),
        "unsupported_command" => [MAGIC, &[0_u8; 8], &[63, 7, 0]].concat(),
        "short_header" => b"PBDEM".to_vec(),
        generator => return Err(format!("unknown synthetic generator {generator}")),
    };
    let error = parse_pinned_demoparser_bytes(&bytes)
        .expect_err("synthetic invalid fixture must not parse");
    if fixture["expected"]["category"].as_str() != Some(error.diagnostic.category.code()) {
        return Err(format!(
            "synthetic fixture {} changed category to {}",
            required_string(fixture, "id")?,
            error.diagnostic.category.code()
        ));
    }
    Ok(())
}

fn read_json(path: &Path) -> Result<Value, String> {
    serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())
}

fn required_string<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value[field]
        .as_str()
        .ok_or_else(|| format!("required string {field} is missing"))
}

fn required_hash(value: &Value, field: &str) -> Result<(), String> {
    if required_string(value, field)?.len() != 64 {
        return Err(format!("{field} must be a SHA-256"));
    }
    Ok(())
}

fn parse_error(error: openfrag_import::ParserError) -> String {
    format!(
        "{} at {}: {}",
        error.diagnostic.category.code(),
        error.diagnostic.stage.code(),
        error.diagnostic.upstream_source
    )
}
