use mu_erd::{parse_args, run};
fn main() {
    if let Err(e) = parse_args().and_then(run) {
        eprintln!("mu-erd: {e}");
        std::process::exit(1);
    }
}
