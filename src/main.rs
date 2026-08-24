mod cli;
mod knowledge;
mod workspace;

fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if let Err(error) = cli::run(arguments) {
        eprintln!("guruterminal-core: {error}");
        std::process::exit(1);
    }
}
