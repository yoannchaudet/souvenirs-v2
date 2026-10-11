use std::io;
use std::time::Duration;

fn main() {
    let options = match engine_options(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("souvenirs-engine: {error}");
            std::process::exit(2);
        }
    };
    if let Err(error) =
        souvenirs_engine::run_engine_with_options(io::stdin().lock(), io::stdout(), options)
    {
        eprintln!("souvenirs-engine: {error}");
        std::process::exit(1);
    }
}

fn engine_options(
    mut arguments: impl Iterator<Item = String>,
) -> Result<souvenirs_engine::EngineOptions, String> {
    let mut options = souvenirs_engine::EngineOptions::default();
    while let Some(argument) = arguments.next() {
        if !cfg!(debug_assertions) || argument != "--test-progress-delay-ms" {
            return Err(format!("unsupported argument {argument:?}"));
        }
        let value = arguments
            .next()
            .ok_or_else(|| "--test-progress-delay-ms requires a value".to_owned())?;
        let milliseconds = value
            .parse::<u64>()
            .map_err(|_| "--test-progress-delay-ms must be an unsigned integer".to_owned())?;
        options.progress_delay = Duration::from_millis(milliseconds);
    }
    Ok(options)
}
