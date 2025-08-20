use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

fn main() -> anyhow::Result<()> {
    // Path from first arg or default to ./id.json
    let path: PathBuf = env::args().nth(1).map(Into::into).unwrap_or_else(|| "id.json".into());

    // Don’t overwrite by accident
    if path.exists() {
        eprintln!("Refusing to overwrite existing file: {}", path.display());
        eprintln!("Pass a different path, or delete the file first.");
        std::process::exit(2);
    }

    // Generate new keypair
    let kp = Keypair::new();
    let secret_bytes: Vec<u8> = kp.to_bytes().to_vec(); // 64 bytes (ed25519 secret + pubkey)

    // Create parent dirs if needed
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    // Open with strict perms (0600 on Unix)
    #[cfg(unix)]
    let mut file = {
        let mut opts = OpenOptions::new();
        opts.create_new(true).write(true).mode(0o600).open(&path)?
    };

    #[cfg(not(unix))]
    let mut file = OpenOptions::new().create_new(true).write(true).open(&path)?;

    // Write JSON array (same format as solana-keygen)
    // Example: [12,34, ... 64 bytes ...]
    let json = serde_json::to_vec(&secret_bytes)?;
    file.write_all(&json)?;
    file.write_all(b"\n")?;

    println!("✅ Keypair written to: {}", path.display());
    println!("🔑 Public key: {}", kp.pubkey());

    Ok(())
}
