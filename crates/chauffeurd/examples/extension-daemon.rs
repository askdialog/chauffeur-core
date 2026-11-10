//! Run the independent package host against an explicit offline payload for SDK/app development.
fn main() -> anyhow::Result<()> {
    let bootstrap = std::env::var_os("CHAUFFEUR_BOOTSTRAP_DIR")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| {
            anyhow::anyhow!("Set CHAUFFEUR_BOOTSTRAP_DIR to the signed bootstrap payload")
        })?;
    chauffeurd::runtime::main(chauffeurd::extensions::managed::registrations(bootstrap))
}
