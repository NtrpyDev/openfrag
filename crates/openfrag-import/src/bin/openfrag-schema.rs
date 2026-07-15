use openfrag_import::normalized_event_schema;
use std::{env, fs, path::Path};

fn main() {
    let args = env::args().collect::<Vec<_>>();
    let result = match args.as_slice() {
        [_, command, path] if command == "--check" => check(Path::new(path)),
        [_, command, path] if command == "--write" => write(Path::new(path)),
        _ => Err("usage: openfrag-schema --check|--write <path>".to_owned()),
    };
    if let Err(error) = result {
        eprintln!("schema command failed: {error}");
        std::process::exit(1);
    }
}

fn generated() -> Result<String, String> {
    let mut output = serde_json::to_string_pretty(&normalized_event_schema())
        .map_err(|error| error.to_string())?;
    output.push('\n');
    Ok(output)
}

fn check(path: &Path) -> Result<(), String> {
    let expected = generated()?;
    let actual = fs::read_to_string(path).map_err(|error| error.to_string())?;
    if actual != expected {
        return Err(format!(
            "{} is stale; run openfrag-schema --write {}",
            path.display(),
            path.display()
        ));
    }
    println!("{} matches generated evidence schema", path.display());
    Ok(())
}

fn write(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(path, generated()?).map_err(|error| error.to_string())?;
    println!("wrote {}", path.display());
    Ok(())
}
