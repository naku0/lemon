use mu_condition_analysis::{parse_args, run};

fn main() {
    if let Err(error) = parse_args().and_then(run) {
        eprintln!("mu-condition-analysis: {error}");
        std::process::exit(1);
    }
}
