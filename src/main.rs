fn main() {
    match git_ai_hook::run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("git-ai-hook: {error:#}");
            std::process::exit(1);
        }
    }
}
