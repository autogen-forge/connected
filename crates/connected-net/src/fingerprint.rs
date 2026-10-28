use std::fmt;
use std::str::FromStr;

use sha2::{Digest, Sha256};

/// SHA-256 of a certificate's DER encoding. This is what a device is known by: trusting a
/// peer means trusting exactly one certificate, identified by its fingerprint.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    pub fn of_certificate(der: &[u8]) -> Self {
        Self(Sha256::digest(der).into())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lowercase hex, 64 characters.
    pub fn to_hex(self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// First 8 bytes in groups of two, for showing next to a device name.
    pub fn short(self) -> String {
        let hex = self.to_hex();
        hex.as_bytes()[..16]
            .chunks(4)
            .map(|chunk| std::str::from_utf8(chunk).unwrap_or(""))
            .collect::<Vec<_>>()
            .join(":")
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({})", self.short())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ParseFingerprintError;

impl fmt::Display for ParseFingerprintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a fingerprint is 64 hexadecimal characters")
    }
}

impl std::error::Error for ParseFingerprintError {}

impl FromStr for Fingerprint {
    type Err = ParseFingerprintError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.len() != 64 || !s.is_ascii() {
            return Err(ParseFingerprintError);
        }
        let mut bytes = [0u8; 32];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&s[index * 2..index * 2 + 2], 16)
                .map_err(|_| ParseFingerprintError)?;
        }
        Ok(Self(bytes))
    }
}

/// The code both people compare when pairing.
///
/// It is derived from both devices' fingerprints, so it is the same on both screens exactly
/// when both ends of the TLS session hold the certificates they think they do. Someone in
/// the middle has to present their own certificate to each side, which gives each side a
/// different code. It has ten digits because an attacker could otherwise grind through
/// certificates until the two codes happen to match: a million tries is seconds of work,
/// ten billion is not something to do while someone is reading the screen.
pub fn pairing_code(a: &Fingerprint, b: &Fingerprint) -> String {
    let (first, second) = if a <= b { (a, b) } else { (b, a) };
    let mut hasher = Sha256::new();
    hasher.update(b"connected pairing code v1");
    hasher.update(first.0);
    hasher.update(second.0);
    let digest = hasher.finalize();

    let value = u64::from_be_bytes(digest[..8].try_into().expect("8 bytes")) % 10_000_000_000;
    format!("{:05} {:05}", value / 100_000, value % 100_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(seed: u8) -> Fingerprint {
        Fingerprint::of_certificate(&[seed; 16])
    }

    #[test]
    fn hex_round_trips() {
        let original = fp(1);
        let parsed: Fingerprint = original.to_hex().parse().unwrap();
        assert_eq!(parsed, original);
        assert_eq!(original.to_hex().len(), 64);
    }

    #[test]
    fn parsing_rejects_bad_input() {
        assert!("".parse::<Fingerprint>().is_err());
        assert!("abcd".parse::<Fingerprint>().is_err());
        assert!("zz".repeat(32).parse::<Fingerprint>().is_err());
        assert!("é".repeat(32).parse::<Fingerprint>().is_err());
    }

    #[test]
    fn different_certificates_have_different_fingerprints() {
        assert_ne!(fp(1), fp(2));
    }

    #[test]
    fn short_form_has_four_groups() {
        assert_eq!(fp(1).short().split(':').count(), 4);
    }

    #[test]
    fn pairing_code_is_the_same_from_both_sides() {
        assert_eq!(pairing_code(&fp(1), &fp(2)), pairing_code(&fp(2), &fp(1)));
    }

    #[test]
    fn pairing_code_depends_on_both_fingerprints() {
        assert_ne!(pairing_code(&fp(1), &fp(2)), pairing_code(&fp(1), &fp(3)));
        assert_ne!(pairing_code(&fp(1), &fp(2)), pairing_code(&fp(3), &fp(2)));
    }

    #[test]
    fn pairing_code_is_ten_digits_in_two_groups() {
        let code = pairing_code(&fp(1), &fp(2));
        let groups: Vec<_> = code.split(' ').collect();
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().all(|g| g.len() == 5 && g.bytes().all(|b| b.is_ascii_digit())));
    }
}
