//! The updater's signatures on Luna's release installers, made apart from
//! the build and checked as installed copies check them
//! (scripts/release-sign.ps1 runs the `luna-updater-signer` binary in a
//! container of its own, with no network and nothing of the build's).
//!
//! Everything is in the form Tauri's signer gave it up to 0.19, which every
//! installed copy reads:
//!
//! - A key pair as `tauri signer generate` writes one: the secret key is
//!   base64 of a minisign secret key file, encrypted with scrypt under a
//!   password that may be empty; the public key is base64 of a minisign
//!   public key file, as the updater embeds it (`src-tauri/updater.json`).
//! - A signature as `tauri signer sign --app-version` writes the `.sig`:
//!   base64 of a minisign signature file, prehashed (Ed25519 over the file's
//!   BLAKE2b-512), its untrusted comment `signature from tauri secret key`,
//!   its trusted comment `timestamp:<unix>\tfile:<name>\tversion:<version>`.
//!
//! Luna's trusted comment adds one field after those, the core fingerprint
//! the release was built with (`core_fingerprint:<16 hex digits>`, build.rs's
//! `core-fingerprint.txt`), so that the updater takes it from what was
//! signed and not from `latest.json`, which is not (desktop/updater.rs
//! `verify`). 0.19's tauri-plugin-updater reads the `version:` field alone
//! (2.14.1, `signed_version`) and passes over the rest.
//!
//! Keys and signing are the `minisign` crate's, as in the Tauri CLI; the
//! check is `minisign-verify`'s, as in both updaters.

use base64::Engine;
use std::io::Read;

/// The untrusted comment Tauri's signer gives every signature. Nothing reads
/// it; it stays as it was so the `.sig` is the one Tauri's signer made.
pub const UNTRUSTED_COMMENT: &str = "signature from tauri secret key";

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// What a release's signature says of its installer, in its trusted comment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    /// The installer's file name.
    pub file: String,
    pub version: String,
    /// build.rs's core fingerprint: 16 lowercase hex digits.
    pub core_fingerprint: String,
}

impl Signed {
    /// The trusted comment saying this, signed at `timestamp` (Unix
    /// seconds): Tauri's three fields in Tauri's order, then the core's.
    /// Refuses a value a reader would split differently from how it was
    /// written (a tab or a line break) and a fingerprint not of build.rs's
    /// form.
    pub fn trusted_comment(&self, timestamp: u64) -> Result<String, String> {
        for (what, value) in [("file name", &self.file), ("version", &self.version)] {
            if value.is_empty() || value.contains(['\t', '\r', '\n']) {
                return Err(format!("the {what} {value:?} cannot go into a trusted comment"));
            }
        }
        if !is_core_fingerprint(&self.core_fingerprint) {
            return Err(format!("{:?} is not a core fingerprint (16 lowercase hex digits)", self.core_fingerprint));
        }
        Ok(format!("timestamp:{timestamp}\tfile:{}\tversion:{}\tcore_fingerprint:{}", self.file, self.version, self.core_fingerprint))
    }

    /// Reads a trusted comment as `trusted_comment` writes one, and nothing
    /// else: its four fields, in its order, each once.
    pub fn parse(comment: &str) -> Result<Signed, String> {
        let not_ours = || format!("the trusted comment {comment:?} is not timestamp, file, version and core_fingerprint");
        let fields: Vec<&str> = comment.split('\t').collect();
        let [timestamp, file, version, core] = fields[..] else { return Err(not_ours()) };
        let timestamp = timestamp.strip_prefix("timestamp:").ok_or_else(not_ours)?;
        if timestamp.is_empty() || !timestamp.bytes().all(|b| b.is_ascii_digit()) {
            return Err(not_ours());
        }
        let signed = Signed {
            file: file.strip_prefix("file:").ok_or_else(not_ours)?.to_owned(),
            version: version.strip_prefix("version:").ok_or_else(not_ours)?.to_owned(),
            core_fingerprint: core.strip_prefix("core_fingerprint:").ok_or_else(not_ours)?.to_owned(),
        };
        if signed.file.is_empty() || signed.version.is_empty() || !is_core_fingerprint(&signed.core_fingerprint) {
            return Err(not_ours());
        }
        Ok(signed)
    }
}

/// A core fingerprint as build.rs writes it: 16 lowercase hex digits.
pub fn is_core_fingerprint(text: &str) -> bool {
    text.len() == 16 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The text base64 `value` encodes, whitespace around it allowed (a key file
/// saved by an editor ends on a line break; the Tauri CLI took none).
fn decode(what: &str, value: &str) -> Result<String, String> {
    let bytes = B64.decode(value.trim()).map_err(|e| format!("{what} is not base64: {e}"))?;
    String::from_utf8(bytes).map_err(|_| format!("{what} is not text"))
}

/// The key id of a minisign public key or signature file: the 8 bytes after
/// the algorithm on its second line, printed as minisign prints them (in
/// reverse, hex), for a message naming the key.
fn key_id(file: &str) -> Option<String> {
    let bin = base64::engine::general_purpose::STANDARD.decode(file.lines().nth(1)?).ok()?;
    Some(bin.get(2..10)?.iter().rev().map(|b| format!("{b:02X}")).collect())
}

/// The secret key `key` holds in Tauri's form (the file `tauri signer
/// generate` wrote, or the release environment's secret), decrypted with
/// `password`: empty for a key made without one, as the maintainer's is.
pub fn secret_key(key: &str, password: &str) -> Result<minisign::SecretKey, String> {
    let text = decode("the secret key", key)?;
    minisign::SecretKeyBox::from_string(&text)
        .and_then(|b| b.into_secret_key(Some(password.to_owned())))
        .map_err(|e| format!("the secret key: {e}"))
}

/// The public half of `key`, in the form the updater embeds it.
pub fn public_key(key: &minisign::SecretKey) -> Result<String, String> {
    let pk = minisign::PublicKey::from_secret_key(key).map_err(|e| format!("the public key: {e}"))?;
    Ok(B64.encode(pk.to_box().map_err(|e| format!("the public key: {e}"))?.to_string()))
}

/// A new key pair in Tauri's form, as `tauri signer generate --password ''`
/// makes one: the secret key, encrypted under an empty password, and the
/// public key.
pub fn generate() -> Result<(String, String), String> {
    let minisign::KeyPair { pk, sk } =
        minisign::KeyPair::generate_encrypted_keypair(Some(String::new())).map_err(|e| format!("making a key: {e}"))?;
    let sk = sk.to_box(None).map_err(|e| format!("making a key: {e}"))?.to_string();
    let pk = pk.to_box().map_err(|e| format!("making a key: {e}"))?.to_string();
    Ok((B64.encode(sk), B64.encode(pk)))
}

/// The `.sig` of `data`: base64 of the minisign signature file, prehashed,
/// with Tauri's untrusted comment and `trusted_comment`, its global signature
/// checked against `key`'s public half before it is returned.
pub fn sign(key: &minisign::SecretKey, data: impl Read, trusted_comment: &str) -> Result<String, String> {
    let pk = minisign::PublicKey::from_secret_key(key).map_err(|e| format!("the public key: {e}"))?;
    let signature =
        minisign::sign(Some(&pk), key, data, Some(trusted_comment), Some(UNTRUSTED_COMMENT)).map_err(|e| format!("signing: {e}"))?;
    Ok(B64.encode(signature.to_string()))
}

/// Checks `signature` (a `.sig`'s text) over `data` against `pubkey` (in the
/// form the updater embeds it) as installed copies check it, and returns its
/// trusted comment: the key id first, then the Ed25519 signature over the
/// file's BLAKE2b-512, then the global signature over that and the trusted
/// comment. Stricter in one thing: a legacy signature, over the file itself,
/// is refused, as every signature made here is prehashed.
pub fn verify(data: &[u8], signature: &str, pubkey: &str) -> Result<String, String> {
    let key_file = decode("the public key", pubkey)?;
    let sig_file = decode("the signature", signature)?;
    let key = minisign_verify::PublicKey::decode(&key_file).map_err(|e| format!("the public key: {e}"))?;
    let sig = minisign_verify::Signature::decode(&sig_file).map_err(|e| format!("the signature: {e}"))?;
    key.verify(data, &sig, false).map_err(|e| match e {
        minisign_verify::Error::UnexpectedKeyId => format!(
            "signed by key {}, not by the updater's {}",
            key_id(&sig_file).unwrap_or_default(),
            key_id(&key_file).unwrap_or_default()
        ),
        minisign_verify::Error::UnexpectedAlgorithm => "a legacy signature, over the file rather than its hash".to_owned(),
        e => format!("the signature does not check out: {e}"),
    })?;
    Ok(sig.trusted_comment().to_owned())
}

/// Checks a release's signature: `verify`, then its trusted comment is one
/// `Signed::trusted_comment` writes and says `expected`. Returns the comment.
pub fn verify_release(data: &[u8], signature: &str, pubkey: &str, expected: &Signed) -> Result<String, String> {
    let comment = verify(data, signature, pubkey)?;
    // Read only now: before the global signature checked out, it was anyone's text.
    let signed = Signed::parse(&comment)?;
    if signed.file != expected.file {
        return Err(format!("signed as {}, not {}", signed.file, expected.file));
    }
    if signed.version != expected.version {
        return Err(format!("signed for version {}, not {}", signed.version, expected.version));
    }
    if signed.core_fingerprint != expected.core_fingerprint {
        return Err(format!("signed for core {}, not {}", signed.core_fingerprint, expected.core_fingerprint));
    }
    Ok(comment)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The fixtures (src-tauri/fixtures/updater/README.md says how each was
    // made), compiled in: the Windows tests run away from the tree.
    const SETUP: &[u8] = include_bytes!("../../../fixtures/updater/setup.bin");
    /// Made by the Tauri CLI 2.12.5 with an empty password, as the
    /// maintainer's key was.
    const TAURI_KEY: &str = include_str!("../../../fixtures/updater/tauri.key");
    const TAURI_PUB: &str = include_str!("../../../fixtures/updater/tauri.key.pub");
    /// Made by the Tauri CLI 2.12.5 with the password `luna fixture password`.
    const TAURI_PASSWORD_KEY: &str = include_str!("../../../fixtures/updater/tauri-password.key");
    const TAURI_PASSWORD_PUB: &str = include_str!("../../../fixtures/updater/tauri-password.key.pub");
    /// A signature the Tauri CLI made, for 0.20.0, with a key since deleted.
    const TAURI_SIG: &str = include_str!("../../../fixtures/updater/setup.bin.sig");
    const TAURI_SIG_PUB: &str = include_str!("../../../fixtures/updater/key.pub");
    /// A key `generate` made, and the signature the Tauri CLI then made with
    /// it, for 0.20.0.
    const SIGNER_PUB: &str = include_str!("../../../fixtures/updater/signer.key.pub");
    const SIGNER_KEY_TAURI_SIG: &str = include_str!("../../../fixtures/updater/setup.bin.signer-key.sig");
    /// What this signer made over setup.bin as Luna_0.20.0_x64-setup.exe
    /// with TAURI_KEY, and tauri-plugin-updater 2.14.1 itself then took for
    /// 0.20.0 and refused for 0.20.1.
    const RELEASE_SIG: &str = include_str!("../../../fixtures/updater/release-0.20.0.sig");
    const CORE: &str = "0123456789abcdef";

    /// What 0.19 runs on an update before it installs it:
    /// tauri-plugin-updater 2.14.1's `verify_signature` and
    /// `verify_signed_version` (src/updater.rs), step for step, on the
    /// minisign-verify and base64 0.19 locks (`the_check_of_0_19_runs_on_its_crates`).
    /// The plugin decodes without trimming; 0.19 leaves `requireSignedVersion`
    /// off (its tauri.conf.json), so there a signature without a version
    /// passes too.
    fn as_0_19_checks(
        data: &[u8],
        release_signature: &str,
        pub_key: &str,
        announced: &str,
        require_signed_version: bool,
    ) -> Result<(), String> {
        let text = |b64: &str| -> Result<String, String> {
            let bytes = base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| e.to_string())?;
            std::str::from_utf8(&bytes).map(str::to_owned).map_err(|e| e.to_string())
        };
        let public_key = minisign_verify::PublicKey::decode(&text(pub_key)?).map_err(|e| e.to_string())?;
        let signature = minisign_verify::Signature::decode(&text(release_signature)?).map_err(|e| e.to_string())?;
        public_key.verify(data, &signature, true).map_err(|e| e.to_string())?;
        let Some(signed) = signature.trusted_comment().split('\t').find_map(|f| f.strip_prefix("version:")) else {
            return if require_signed_version { Err("no signed version".into()) } else { Ok(()) };
        };
        let same = match (semver::Version::parse(signed.trim_start_matches('v')), semver::Version::parse(announced.trim_start_matches('v')))
        {
            (Ok(s), Ok(a)) => s == a,
            _ => signed == announced,
        };
        if same { Ok(()) } else { Err(format!("signed {signed}, announced {announced}")) }
    }

    fn release(version: &str) -> Signed {
        Signed { file: "Luna_0.20.0_x64-setup.exe".into(), version: version.into(), core_fingerprint: CORE.into() }
    }

    fn signed_by(key: &str, password: &str, what: &Signed) -> String {
        let key = secret_key(key, password).unwrap();
        sign(&key, SETUP, &what.trusted_comment(1_795_000_000).unwrap()).unwrap()
    }

    /// The four lines of a `.sig`, decoded as 0.19 decodes it: no trimming.
    fn lines(sig: &str) -> Vec<String> {
        let text = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(sig).unwrap()).unwrap();
        assert!(text.ends_with('\n') && !text.contains('\r'), "{text:?}");
        text.lines().map(str::to_owned).collect()
    }

    #[test]
    fn a_key_the_tauri_cli_made_signs_what_0_19_and_this_check_take() {
        let sig = signed_by(TAURI_KEY, "", &release("0.20.0"));
        for require in [false, true] {
            assert_eq!(as_0_19_checks(SETUP, &sig, TAURI_PUB, "0.20.0", require), Ok(()));
            assert_eq!(as_0_19_checks(SETUP, &sig, TAURI_PUB, "v0.20.0", require), Ok(()));
            assert!(as_0_19_checks(SETUP, &sig, TAURI_PUB, "0.20.1", require).is_err(), "another version announced");
            assert!(as_0_19_checks(b"other bytes", &sig, TAURI_PUB, "0.20.0", require).is_err());
            assert!(as_0_19_checks(SETUP, &sig, TAURI_SIG_PUB, "0.20.0", require).is_err(), "another key");
        }
        let comment = verify_release(SETUP, &sig, TAURI_PUB, &release("0.20.0")).unwrap();
        assert_eq!(comment, "timestamp:1795000000\tfile:Luna_0.20.0_x64-setup.exe\tversion:0.20.0\tcore_fingerprint:0123456789abcdef");
        // Each signature is new: minisign 0.10 hedges Ed25519 with fresh noise.
        assert_ne!(sig, signed_by(TAURI_KEY, "", &release("0.20.0")));
    }

    #[test]
    fn a_password_on_the_key_is_taken_and_a_wrong_one_refused() {
        let sig = signed_by(TAURI_PASSWORD_KEY, "luna fixture password", &release("0.20.0"));
        assert_eq!(as_0_19_checks(SETUP, &sig, TAURI_PASSWORD_PUB, "0.20.0", true), Ok(()));
        for wrong in ["", "luna fixture password ", "LUNA FIXTURE PASSWORD"] {
            assert!(secret_key(TAURI_PASSWORD_KEY, wrong).unwrap_err().contains("Wrong password"), "{wrong:?}");
        }
        assert!(secret_key(TAURI_KEY, "a password").unwrap_err().contains("Wrong password"));
        // A key file with a line break after it, as an editor saves one.
        assert!(secret_key(&format!("{TAURI_KEY}\n"), "").is_ok());
        assert!(secret_key("not a key", "").unwrap_err().contains("not base64"));
    }

    #[test]
    fn the_signature_is_in_the_form_tauris_signer_made() {
        let sig = signed_by(TAURI_KEY, "", &release("0.20.0"));
        // Canonical base64, nothing around it: 0.19 decodes latest.json's as it is.
        assert_eq!(sig.trim(), sig);
        let ours = lines(&sig);
        let tauris = lines(TAURI_SIG);
        assert_eq!(ours.len(), 4);
        assert_eq!(tauris.len(), 4);
        assert_eq!(ours[0], tauris[0]);
        assert_eq!(ours[0], format!("untrusted comment: {UNTRUSTED_COMMENT}"));
        let b64 = |line: &str| base64::engine::general_purpose::STANDARD.decode(line).unwrap();
        // Prehashed ("ED"), signed by the key whose id the public key carries.
        let key_file = String::from_utf8(b64(TAURI_PUB)).unwrap();
        let (bin, pubkey) = (b64(&ours[1]), b64(key_file.lines().nth(1).unwrap()));
        assert_eq!((bin.len(), &bin[..2], &bin[2..10]), (74, &b"ED"[..], &pubkey[2..10]));
        assert_eq!(&b64(&tauris[1])[..2], b"ED");
        // Tauri's fields first, in Tauri's order, then the core's.
        let fields = |line: &str| {
            line.strip_prefix("trusted comment: ").unwrap().split('\t').map(|f| f.split(':').next().unwrap().to_owned()).collect::<Vec<_>>()
        };
        assert_eq!(fields(&tauris[2]), ["timestamp", "file", "version"]);
        assert_eq!(fields(&ours[2]), ["timestamp", "file", "version", "core_fingerprint"]);
        assert_eq!(b64(&ours[3]).len(), 64);
    }

    #[test]
    fn a_key_made_here_is_in_tauris_form_and_signs() {
        let (secret, public) = generate().unwrap();
        let secret_file = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(&secret).unwrap()).unwrap();
        let tauri_file = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(TAURI_KEY).unwrap()).unwrap();
        assert_eq!(secret_file.lines().next(), tauri_file.lines().next());
        assert_eq!(secret_file.lines().nth(1).unwrap().len(), tauri_file.lines().nth(1).unwrap().len());
        let public_file = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(&public).unwrap()).unwrap();
        assert!(public_file.starts_with("untrusted comment: minisign public key: "), "{public_file}");
        // Encrypted under an empty password, as the Tauri CLI's `--password ''`.
        assert!(secret_key(&secret, "x").is_err());
        let key = secret_key(&secret, "").unwrap();
        assert_eq!(public_key(&key).unwrap(), public);
        assert_eq!(public_key(&secret_key(TAURI_KEY, "").unwrap()).unwrap(), TAURI_PUB.trim());
        let sig = sign(&key, SETUP, &release("0.20.0").trusted_comment(1).unwrap()).unwrap();
        assert_eq!(as_0_19_checks(SETUP, &sig, &public, "0.20.0", true), Ok(()));
    }

    /// The signature the real plugin took is one this signer makes, and the
    /// check above, 0.19's step for step, says of it what the plugin said.
    #[test]
    fn the_signature_the_real_plugin_took_is_one_made_here() {
        let release = Signed { file: "Luna_0.20.0_x64-setup.exe".into(), version: "0.20.0".into(), core_fingerprint: CORE.into() };
        let comment = verify_release(SETUP, RELEASE_SIG, TAURI_PUB, &release).unwrap();
        assert!(comment.starts_with("timestamp:1795158621\tfile:Luna_0.20.0_x64-setup.exe\t"), "{comment}");
        assert_eq!(lines(RELEASE_SIG.trim())[0], format!("untrusted comment: {UNTRUSTED_COMMENT}"));
        assert_eq!(as_0_19_checks(SETUP, RELEASE_SIG.trim(), TAURI_PUB, "0.20.0", true), Ok(()));
        assert!(as_0_19_checks(SETUP, RELEASE_SIG.trim(), TAURI_PUB, "0.20.1", true).is_err());
    }

    #[test]
    fn a_key_made_here_is_one_the_tauri_cli_signs_with() {
        assert_eq!(as_0_19_checks(SETUP, SIGNER_KEY_TAURI_SIG, SIGNER_PUB, "0.20.0", true), Ok(()));
        let comment = verify(SETUP, SIGNER_KEY_TAURI_SIG, SIGNER_PUB).unwrap();
        assert!(comment.ends_with("\tfile:setup.bin\tversion:0.20.0"), "{comment}");
    }

    #[test]
    fn a_signature_the_tauri_cli_made_checks_out_here_but_is_no_release_of_ours() {
        let comment = verify(SETUP, TAURI_SIG, TAURI_SIG_PUB).unwrap();
        assert!(comment.ends_with("\tfile:setup.bin\tversion:0.20.0"), "{comment}");
        let ours = Signed { file: "setup.bin".into(), version: "0.20.0".into(), core_fingerprint: CORE.into() };
        assert!(
            verify_release(SETUP, TAURI_SIG, TAURI_SIG_PUB, &ours)
                .unwrap_err()
                .contains("is not timestamp, file, version and core_fingerprint")
        );
    }

    #[test]
    fn a_release_signature_names_its_file_version_and_core() {
        let sig = signed_by(TAURI_KEY, "", &release("0.20.0"));
        let mut other = release("0.20.0");
        other.file = "Luna_0.20.1_x64-setup.exe".into();
        assert!(verify_release(SETUP, &sig, TAURI_PUB, &other).unwrap_err().contains("signed as Luna_0.20.0_x64-setup.exe"));
        assert!(verify_release(SETUP, &sig, TAURI_PUB, &release("0.20.1")).unwrap_err().contains("signed for version 0.20.0, not 0.20.1"));
        let mut core = release("0.20.0");
        core.core_fingerprint = "fedcba9876543210".into();
        assert!(verify_release(SETUP, &sig, TAURI_PUB, &core).unwrap_err().contains("signed for core 0123456789abcdef"));
        // The comment edited after signing: the global signature no longer holds.
        let text = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(&sig).unwrap()).unwrap();
        let edited = B64.encode(text.replace(CORE, "fedcba9876543210"));
        assert!(verify_release(SETUP, &edited, TAURI_PUB, &core).unwrap_err().contains("does not check out"));
        assert!(as_0_19_checks(SETUP, &edited, TAURI_PUB, "0.20.0", false).is_err());
        // Another key, named by its id.
        let err = verify(SETUP, &sig, TAURI_PASSWORD_PUB).unwrap_err();
        assert_eq!(err, "signed by key DFE6F392E64A3EA9, not by the updater's A115BC8E054618D1");
        assert!(verify(b"other bytes", &sig, TAURI_PUB).unwrap_err().contains("does not check out"));
        assert!(verify(SETUP, "not base64!", TAURI_PUB).unwrap_err().contains("not base64"));
    }

    #[test]
    fn what_a_trusted_comment_cannot_carry_is_refused() {
        let with = |file: &str, version: &str, core: &str| {
            Signed { file: file.into(), version: version.into(), core_fingerprint: core.into() }.trusted_comment(1)
        };
        assert!(with("a\tb.exe", "1.0.0", CORE).is_err());
        assert!(with("a.exe", "1.0.0\n", CORE).is_err());
        assert!(with("", "1.0.0", CORE).is_err());
        assert!(with("a.exe", "1.0.0", "0123456789ABCDEF").is_err());
        assert!(with("a.exe", "1.0.0", "0123456789abcde").is_err());
        let comment = with("a.exe", "1.0.0-rc.1", CORE).unwrap();
        assert_eq!(
            Signed::parse(&comment),
            Ok(Signed { file: "a.exe".into(), version: "1.0.0-rc.1".into(), core_fingerprint: CORE.into() })
        );
        for other in [
            "timestamp:1\tfile:a.exe\tversion:1.0.0",
            "timestamp:1\tfile:a.exe\tversion:1.0.0\tcore_fingerprint:0123456789abcdef\tx:y",
            "timestamp:1\tversion:1.0.0\tfile:a.exe\tcore_fingerprint:0123456789abcdef",
            "timestamp:\tfile:a.exe\tversion:1.0.0\tcore_fingerprint:0123456789abcdef",
        ] {
            assert!(Signed::parse(other).is_err(), "{other}");
        }
    }

    /// The check above stands for 0.19's only while it runs on the crates
    /// 0.19 locks: minisign-verify 0.3.0 and base64 0.23 (v0.19.0's
    /// src-tauri/Cargo.lock, under tauri-plugin-updater 2.14.1). A bump here
    /// needs a copy of 0.19's to check against instead.
    #[test]
    fn the_check_of_0_19_runs_on_its_crates() {
        let lock = include_str!("../../../Cargo.lock").replace("\r\n", "\n");
        let versions = |name: &str| -> Vec<String> {
            lock.split("\n[[package]]\n")
                .filter_map(|p| p.strip_prefix(&format!("name = \"{name}\"\nversion = \"")))
                .map(|rest| rest[..rest.find('"').unwrap()].to_owned())
                .collect()
        };
        assert_eq!(versions("minisign-verify"), ["0.3.0"]);
        assert!(versions("base64").iter().any(|v| v.starts_with("0.23.")), "{:?}", versions("base64"));
    }
}
