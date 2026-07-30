fn main() {
    if let Err(e) = obfuscation_cli::run_cli() {
        eprintln!("错误: {e:#}");
        std::process::exit(1);
    }
}
