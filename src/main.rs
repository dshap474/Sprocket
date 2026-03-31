fn main() {
    let exit_code = match sprocket::run(std::env::args()) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    };
    std::process::exit(exit_code);
}
