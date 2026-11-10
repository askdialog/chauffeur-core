//! Writes `protocol-ts/src/generated/methods.ts`; `scripts/gen-protocol.sh` runs it.

use chauffeur_protocol::generate::{METHODS_TS, methods_ts};

fn main() -> std::io::Result<()> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(METHODS_TS);
    std::fs::create_dir_all(path.parent().expect("a file has a folder"))?;
    std::fs::write(path, methods_ts())
}
