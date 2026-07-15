use openfrag_import::{ParsedOutput, parse_with_pinned_demoparser_with_progress, query_plan_hash};
use serde_json::Value;
use std::{env, fs, path::Path, time::Instant};

fn main() {
    let args = env::args().collect::<Vec<_>>();
    if args.len() < 3 || args.len() > 4 {
        eprintln!(
            "usage: openfrag-parser-benchmark <fixture.dem> <fixture-manifest.json> [iterations]"
        );
        std::process::exit(2);
    }
    let fixture_path = Path::new(&args[1]);
    let manifest_path = Path::new(&args[2]);
    let iterations = args
        .get(3)
        .map_or(Ok(3_usize), |value| value.parse::<usize>())
        .expect("iterations must be an integer");
    assert!(iterations > 0, "iterations must be positive");

    let manifest: Value = serde_json::from_slice(
        &fs::read(manifest_path).expect("fixture manifest must be readable"),
    )
    .expect("fixture manifest must be valid JSON");
    let expected_query_hash = manifest["query_plan_hash"]
        .as_str()
        .expect("manifest query_plan_hash must be a string");
    assert_eq!(
        query_plan_hash(),
        expected_query_hash,
        "the exact OpenFrag query plan changed without a fixture manifest update"
    );

    let mut canonical: Option<ParsedOutput> = None;
    let mut elapsed_seconds = Vec::with_capacity(iterations);
    let mut final_frames = 0_u64;
    let mut final_events = 0_u64;
    for _ in 0..iterations {
        let started = Instant::now();
        let parsed = parse_with_pinned_demoparser_with_progress(fixture_path, &mut |progress| {
            final_frames = final_frames.max(progress.frames);
            final_events = final_events.max(progress.events_emitted);
        })
        .expect("fixed fixture must parse");
        elapsed_seconds.push(started.elapsed().as_secs_f64());
        if let Some(expected) = &canonical {
            assert_eq!(
                expected, &parsed,
                "normalized parser output is not deterministic"
            );
        } else {
            canonical = Some(parsed);
        }
    }
    let parsed = canonical.expect("at least one parse must complete");
    let fixtures = manifest["fixtures"]
        .as_array()
        .expect("manifest fixtures must be an array");
    let fixture = fixtures
        .iter()
        .find(|fixture| fixture["sha256"].as_str() == Some(&parsed.identity.source_sha256))
        .expect("fixture SHA-256 is not present in the fixed manifest");
    for (field, observed) in [
        ("participants", parsed.participants.len() as u64),
        ("rounds", parsed.rounds.len() as u64),
        ("normalized_events", parsed.events.len() as u64),
        ("player_snapshots", parsed.player_snapshots.len() as u64),
    ] {
        assert_eq!(
            fixture[field].as_u64(),
            Some(observed),
            "fixed fixture output count changed for {field}"
        );
    }
    elapsed_seconds.sort_by(f64::total_cmp);
    let total_seconds = elapsed_seconds.iter().sum::<f64>();
    let median_seconds = elapsed_seconds[elapsed_seconds.len() / 2];
    let throughput = parsed.events.len() as f64 * iterations as f64 / total_seconds;
    println!("fixture_id={}", fixture["id"].as_str().unwrap_or("unknown"));
    println!("fixture_sha256={}", parsed.identity.source_sha256);
    println!("query_plan_hash={}", parsed.identity.requested_schema_hash);
    println!("iterations={iterations}");
    println!("median_wall_seconds={median_seconds:.6}");
    println!("normalized_events_per_second={throughput:.3}");
    println!("normalized_events={}", parsed.events.len());
    println!("parser_frames={final_frames}");
    println!("parser_emitted_events={final_events}");
    println!("deterministic=true");
}
