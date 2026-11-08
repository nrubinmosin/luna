//! The detached OpenPGP signature Anthropic publishes beside each Claude Code
//! release manifest (`manifest.json.sig`), checked against the release signing
//! key pinned here, as the setup docs do it with gpg
//! (<https://code.claude.com/docs/en/setup#verify-the-manifest-signature>).
//! The manifest carries the exe's sha256, so a signed manifest is what ties
//! the exe to Anthropic rather than to whoever serves the bucket.
//!
//! Only the shape that key makes is taken: one v4 signature packet, served
//! ASCII-armored, over a binary document, RSA, SHA-512. Anything else is
//! refused rather than parsed further, which keeps this a page instead of an
//! OpenPGP implementation. `ring`, in the build already for TLS, does the RSA.

use base64::Engine;
use ring::signature::{RsaPublicKeyComponents, RSA_PKCS1_2048_8192_SHA512};

/// The modulus of "Anthropic Claude Code Release Signing
/// <security@anthropic.com>", RSA 4096, created 2026-03-30, fingerprint
/// 31DD DE24 DDFA B679 F42D 7BD2 BAA9 29FF 1A7E CACE (the test below derives
/// the fingerprint from it). A new key reaches Luna only with a Luna release.
const KEY_N: &str = "a76f2b495e48f0798af022bd1a2c4151947f82d27178e0ac4573b810989e8f8d23cc19900377165c6a2c54ef3665f16c\
44a553b4c0b9881fc71fdb318e772b9983561aba0935bc76f66fc58184018240974f801f165eab9e75ea4462d939eae3ff802a138c90d4665b\
841811e7967bae8d2278a2bc447614b2904a0051f25918420b3e931d6e9845b7f4a7f846ff4f13398e928f7709eff004a1345ff59995357d64\
d0abdb5128951a9198f4e2075a0c51254176a40ad4e28fe193eb2a180804521f018ef8f0bf3ebeaae2d73e0fb2d18f1481bd4583a054a696a6\
ff3c9d618fd0ffec8d8e7b85d79e200f58c479e8abf11e5cceceb350749ba36c9752902c21d598a3912777cbc6b25b959851319d45a6fdf4d5\
306a0ce569b7bf55517a93327449c9f9ab73e2841d98f41c28d022a791893b6bb293efb116bf42a4abe54798eb6a5f76b68615fe6fa6babdef\
6bcc9d6a6581a3fa86effc2dd2c752f352301fa95cbb4490dfdae0283f9b7683cbbf4e44a953184c5448d90644b161984ccb340ceadf088245\
1f9d2ac83d31e2c5b0d60116786de30e063c723d92b59cfece3ab351dd390a26dc0b31001a6d44420e72f43e34b48b00535388edba6043d574\
942f25aea13bcc963ae942bd67575a03c8eb7438ba9a280d8c716b5df4f757842f32d84220ed39ab4e524b7c761b7669220988f8278f50f075\
a437fe9613e45353";
const KEY_E: [u8; 3] = [0x01, 0x00, 0x01];

/// What the record says checked the manifest.
pub const KEY_NAME: &str = "release key 31DDDE24DDFAB679F42D7BD2BAA929FF1A7ECACE";

const TAG_SIGNATURE: u8 = 2;
const SIG_BINARY_DOCUMENT: u8 = 0x00;
const PUBKEY_RSA: u8 = 1;
const HASH_SHA512: u8 = 10;
/// Subpackets a signature may mark critical and still be understood here:
/// creation time, issuer key ID, issuer fingerprint.
const KNOWN_CRITICAL: [u8; 3] = [2, 16, 33];

/// Whether `sig` is the pinned key's signature over exactly `manifest`.
pub fn verify(manifest: &[u8], sig: &[u8]) -> Result<(), String> {
    let packet = dearmor(sig)?;
    let body = packet_body(&packet, TAG_SIGNATURE)?;
    let bad = |what: &str| format!("manifest signature: {what}");
    let [version, kind, key_algo, hash_algo, ..] = *body else { return Err(bad("too short")) };
    if (version, kind, key_algo, hash_algo) != (4, SIG_BINARY_DOCUMENT, PUBKEY_RSA, HASH_SHA512) {
        return Err(bad(&format!("not a v4 RSA/SHA-512 document signature ({version}, {kind}, {key_algo}, {hash_algo})")));
    }
    let hashed_end = 6 + u16_at(body, 4).ok_or_else(|| bad("too short"))? as usize;
    check_subpackets(body.get(6..hashed_end).ok_or_else(|| bad("hashed subpackets cut short"))?).map_err(|e| bad(&e))?;
    let unhashed_end = hashed_end + 2 + u16_at(body, hashed_end).ok_or_else(|| bad("too short"))? as usize;
    // Two bytes of the digest, for a quick reject; the RSA check covers them.
    let mpi_at = unhashed_end + 2;
    let bits = u16_at(body, mpi_at).ok_or_else(|| bad("no signature value"))? as usize;
    let value = body.get(mpi_at + 2..).filter(|v| v.len() == bits.div_ceil(8)).ok_or_else(|| bad("signature value cut short"))?;

    let n = unhex(KEY_N);
    // An MPI drops leading zero bytes; RSA wants the value at the modulus's length.
    let mut padded = vec![0u8; n.len()];
    let pad = n.len().checked_sub(value.len()).ok_or_else(|| bad("signature value longer than the key"))?;
    padded[pad..].copy_from_slice(value);

    // What a v4 signature signs: the document, the signature's hashed part,
    // and a trailer giving that part's length.
    let mut signed = Vec::with_capacity(manifest.len() + hashed_end + 6);
    signed.extend_from_slice(manifest);
    signed.extend_from_slice(&body[..hashed_end]);
    signed.extend_from_slice(&[4, 0xff]);
    signed.extend_from_slice(&(hashed_end as u32).to_be_bytes());
    RsaPublicKeyComponents { n: &n[..], e: &KEY_E[..] }
        .verify(&RSA_PKCS1_2048_8192_SHA512, &signed, &padded)
        .map_err(|_| bad("does not verify against the pinned release key"))
}

/// The packet inside an armored signature, as the bucket serves it; a binary
/// one as it is. The armor's CRC is not checked: the RSA check covers every
/// byte that matters.
fn dearmor(sig: &[u8]) -> Result<Vec<u8>, String> {
    const BEGIN: &str = "-----BEGIN PGP SIGNATURE-----";
    const END: &str = "-----END PGP SIGNATURE-----";
    let Some(text) = std::str::from_utf8(sig).ok().filter(|t| t.trim_start().starts_with(BEGIN)) else {
        return Ok(sig.to_vec());
    };
    let mut lines = text.lines().map(str::trim).skip_while(|l| *l != BEGIN).skip(1);
    // Armor headers (`Version: ...`) end at the first blank line.
    for line in lines.by_ref() {
        if line.is_empty() {
            break;
        }
        if !line.contains(": ") {
            return Err("manifest signature: armor header without its blank line".into());
        }
    }
    let mut b64 = String::new();
    let mut ended = false;
    for line in lines {
        if line == END {
            ended = true;
            break;
        }
        if !line.starts_with('=') {
            b64.push_str(line);
        }
    }
    if !ended {
        return Err("manifest signature: armor without its end line".into());
    }
    base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| format!("manifest signature: {e}"))
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

/// The body of the one packet `data` must be, of `tag`, in either header
/// format; a partial length or anything after the packet is refused.
fn packet_body(data: &[u8], tag: u8) -> Result<&[u8], String> {
    let bad = |what: &str| Err(format!("manifest signature: {what}"));
    let Some((&ctb, rest)) = data.split_first() else { return bad("empty") };
    if ctb & 0x80 == 0 {
        return bad("not a binary OpenPGP packet");
    }
    let be = |b: &[u8]| b.iter().fold(0usize, |acc, &x| (acc << 8) | x as usize);
    let (got, header, len) = if ctb & 0x40 != 0 {
        match rest.first() {
            Some(&o) if o < 192 => (ctb & 0x3f, 1, o as usize),
            Some(&o) if o < 224 && rest.len() > 1 => (ctb & 0x3f, 2, ((o as usize - 192) << 8) + rest[1] as usize + 192),
            Some(255) if rest.len() > 4 => (ctb & 0x3f, 5, be(&rest[1..5])),
            _ => return bad("a partial or cut-short packet length"),
        }
    } else {
        let width = match ctb & 3 {
            0 => 1,
            1 => 2,
            2 => 4,
            _ => return bad("a packet of indeterminate length"),
        };
        if rest.len() < width {
            return bad("cut short");
        }
        ((ctb >> 2) & 0x0f, width, be(&rest[..width]))
    };
    if got != tag {
        return bad(&format!("packet of type {got}, not a signature"));
    }
    match rest.get(header..) {
        Some(body) if body.len() == len => Ok(body),
        _ => bad("packet length does not match the file"),
    }
}

/// Refuses a critical subpacket this check does not understand: a signature
/// that says "do not accept me unless you know this" is not accepted blind.
fn check_subpackets(mut area: &[u8]) -> Result<(), String> {
    while !area.is_empty() {
        let (len, header) = match area[0] {
            o if o < 192 => (o as usize, 1),
            o if o < 255 && area.len() > 1 => (((o as usize - 192) << 8) + area[1] as usize + 192, 2),
            255 if area.len() > 4 => (u32::from_be_bytes([area[1], area[2], area[3], area[4]]) as usize, 5),
            _ => return Err("subpacket length cut short".into()),
        };
        let packet = area.get(header..header + len).filter(|p| !p.is_empty()).ok_or("subpacket cut short")?;
        let (critical, kind) = (packet[0] & 0x80 != 0, packet[0] & 0x7f);
        if critical && !KNOWN_CRITICAL.contains(&kind) {
            return Err(format!("critical subpacket {kind} not understood"));
        }
        area = &area[header + len..];
    }
    Ok(())
}

fn unhex(s: &str) -> Vec<u8> {
    s.as_bytes().chunks(2).map(|pair| u8::from_str_radix(std::str::from_utf8(pair).expect("ascii"), 16).expect("hex")).collect()
}

#[cfg(test)]
mod tests {
    use super::{dearmor, unhex, verify, KEY_E, KEY_N};

    // A real release's pair, as downloads.claude.ai served it, the signature
    // armored (.gitattributes keeps their bytes).
    const MANIFEST: &[u8] = include_bytes!("testdata/manifest-2.1.319.json");
    const SIG: &[u8] = include_bytes!("testdata/manifest-2.1.319.json.sig");

    #[test]
    fn a_released_manifest_verifies_armored_or_not() {
        verify(MANIFEST, SIG).unwrap();
        verify(MANIFEST, &dearmor(SIG).unwrap()).unwrap();
    }

    #[test]
    fn a_changed_manifest_or_signature_does_not() {
        // Another checksum for the exe, as a swapped bucket would serve it.
        let key = "\"checksum\": \"";
        let at = std::str::from_utf8(MANIFEST).unwrap().find(key).unwrap() + key.len();
        let mut changed = MANIFEST.to_vec();
        changed[at] = if changed[at] == b'0' { b'1' } else { b'0' };
        assert!(verify(&changed, SIG).unwrap_err().contains("does not verify"));

        let packet = dearmor(SIG).unwrap();
        // The signature value's last byte, and a hashed subpacket's.
        for at in [packet.len() - 1, 12] {
            let mut forged = packet.clone();
            forged[at] ^= 1;
            assert!(verify(MANIFEST, &forged).is_err(), "byte {at} changed");
        }
        assert!(verify(MANIFEST, &packet[..packet.len() - 1]).is_err());
        let text = std::str::from_utf8(SIG).unwrap();
        assert!(verify(MANIFEST, text.replace("-----END PGP SIGNATURE-----", "").as_bytes()).is_err());
        assert!(verify(MANIFEST, b"-----BEGIN PGP SIGNATURE-----").is_err());
        assert!(verify(MANIFEST, b"").is_err());
    }

    /// The pinned modulus is the documented key: its v4 fingerprint, SHA-1
    /// over the public key packet rebuilt from it, is the one the setup docs
    /// give.
    #[test]
    fn the_pinned_key_has_the_documented_fingerprint() {
        fn mpi(v: &[u8]) -> Vec<u8> {
            let bits = v.len() * 8 - v[0].leading_zeros() as usize;
            let mut out = (bits as u16).to_be_bytes().to_vec();
            out.extend_from_slice(v);
            out
        }
        let mut body = vec![4u8];
        body.extend_from_slice(&1_774_907_248u32.to_be_bytes());
        body.push(1);
        body.extend(mpi(&unhex(KEY_N)));
        body.extend(mpi(&KEY_E));
        let mut packet = vec![0x99];
        packet.extend_from_slice(&(body.len() as u16).to_be_bytes());
        packet.extend(body);
        let digest = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, &packet);
        let hex: String = digest.as_ref().iter().map(|b| format!("{b:02X}")).collect();
        assert_eq!(hex, "31DDDE24DDFAB679F42D7BD2BAA929FF1A7ECACE");
    }
}
