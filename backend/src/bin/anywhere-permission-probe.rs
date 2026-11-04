//! Independent of the running Host service, so UI updates can retain a newer backend.
#[path = "../permissions.rs"]
mod permissions;

fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    anyhow::ensure!(
        args.len() == 3 && args[1] == "--check-directory-access",
        "expected --check-directory-access <path>"
    );
    println!(
        "{}",
        serde_json::to_string(&permissions::check(std::path::Path::new(&args[2])))?
    );
    Ok(())
}
