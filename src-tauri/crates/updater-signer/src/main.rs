//! `luna-updater-signer`: the release's updater signatures (lib.rs says in
//! what form), as scripts/release-sign.ps1 runs it in its containers.
//!
//!   luna-updater-signer sign <file> <version> <core-fingerprint>
//!   luna-updater-signer generate <keyfile>
//!   luna-updater-signer verify <file> <file.sig> <pubkey> <version> <core-fingerprint>
//!
//! sign takes the key from the environment, under the names the Tauri CLI
//! took it by, which the release environment's secrets keep:
//! TAURI_SIGNING_PRIVATE_KEY_PATH, a file holding it, or
//! TAURI_SIGNING_PRIVATE_KEY, the key itself, and
//! TAURI_SIGNING_PRIVATE_KEY_PASSWORD, its password, empty when unset (as the
//! Tauri CLI took it with no terminal to ask at). It writes <file>.sig, which
//! must not be there yet; the trusted comment names <file>'s name, <version>
//! and <core-fingerprint>.
//!
//! generate writes a new key pair in Tauri's form with an empty password,
//! <keyfile> and <keyfile>.pub, neither there yet: what a dry run signs with.
//!
//! verify checks <file.sig> over <file> against <pubkey> (in the form the
//! updater embeds it) as installed copies check it, and that its trusted
//! comment names <file>'s name, <version> and <core-fingerprint>; it prints
//! the comment.

use luna_updater_signer::{Signed, generate, secret_key, sign, verify_release};
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage: luna-updater-signer sign <file> <version> <core-fingerprint> | generate <keyfile> | verify <file> <file.sig> <pubkey> <version> <core-fingerprint>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let done = match args[..] {
        ["sign", file, version, core] => sign_file(Path::new(file), version, core),
        ["generate", keyfile] => generate_key(Path::new(keyfile)),
        ["verify", file, sig, pubkey, version, core] => verify_file(Path::new(file), Path::new(sig), pubkey, version, core),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("luna-updater-signer: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The file name a trusted comment names `file` by.
fn name_of(file: &Path) -> Result<String, String> {
    file.file_name().and_then(|n| n.to_str()).map(str::to_owned).ok_or_else(|| format!("{} names no file", file.display()))
}

/// Creates `path`, which must not be there yet, and writes `text` into it;
/// `private`, readable by its owner alone where the OS has modes.
fn write_new(path: &Path, text: &str, private: bool) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut file = options.open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(text.as_bytes()).and_then(|()| file.sync_all()).map_err(|e| format!("{}: {e}", path.display()))
}

fn sign_file(file: &Path, version: &str, core: &str) -> Result<(), String> {
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let key = match (env("TAURI_SIGNING_PRIVATE_KEY_PATH"), env("TAURI_SIGNING_PRIVATE_KEY")) {
        (Some(path), None) => std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?,
        (None, Some(key)) => key,
        (Some(_), Some(_)) => return Err("both TAURI_SIGNING_PRIVATE_KEY_PATH and TAURI_SIGNING_PRIVATE_KEY are set; set one".into()),
        (None, None) => return Err("no key: set TAURI_SIGNING_PRIVATE_KEY_PATH or TAURI_SIGNING_PRIVATE_KEY".into()),
    };
    let password = std::env::var("TAURI_SIGNING_PRIVATE_KEY_PASSWORD").unwrap_or_default();
    let key = secret_key(&key, &password)?;
    let comment = Signed { file: name_of(file)?, version: version.to_owned(), core_fingerprint: core.to_owned() }.trusted_comment(now()?)?;
    let data = std::fs::File::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let signature = sign(&key, std::io::BufReader::new(data), &comment)?;
    let mut sig_path = file.as_os_str().to_owned();
    sig_path.push(".sig");
    let sig_path = Path::new(&sig_path);
    write_new(sig_path, &signature, false)?;
    println!("signed {}: {}", name_of(file)?, comment.replace('\t', " "));
    Ok(())
}

fn now() -> Result<u64, String> {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).map_err(|e| format!("the clock: {e}"))
}

fn generate_key(keyfile: &Path) -> Result<(), String> {
    let (secret, public) = generate()?;
    let mut pub_path = keyfile.as_os_str().to_owned();
    pub_path.push(".pub");
    let pub_path = Path::new(&pub_path);
    if pub_path.exists() {
        return Err(format!("{} is there already", pub_path.display()));
    }
    write_new(keyfile, &secret, true)?;
    write_new(pub_path, &public, false)?;
    println!("made a key pair: {} and {}", keyfile.display(), pub_path.display());
    Ok(())
}

fn verify_file(file: &Path, sig: &Path, pubkey: &str, version: &str, core: &str) -> Result<(), String> {
    let data = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let signature = std::fs::read_to_string(sig).map_err(|e| format!("{}: {e}", sig.display()))?;
    let expected = Signed { file: name_of(file)?, version: version.to_owned(), core_fingerprint: core.to_owned() };
    let comment = verify_release(&data, &signature, pubkey, &expected)?;
    println!("{} verifies against the updater's public key: {}", name_of(sig)?, comment.replace('\t', " "));
    Ok(())
}
