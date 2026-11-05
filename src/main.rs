use std::io::{self, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let stdin = io::stdin();
    let stdout = io::stdout();
    let stderr = io::stderr();
    let code = diffmerge::cli::run(&args, &mut stdin.lock(), &mut stdout.lock(), &mut stderr.lock());
    let _ = io::stdout().flush();
    std::process::exit(code);
}
