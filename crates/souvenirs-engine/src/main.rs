use std::io;

fn main() {
    if let Err(error) = souvenirs_engine::run_engine(io::stdin().lock(), io::stdout().lock()) {
        eprintln!("souvenirs-engine: {error}");
        std::process::exit(1);
    }
}
