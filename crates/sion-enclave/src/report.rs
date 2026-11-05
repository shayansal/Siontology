//! An AMD SEV-SNP attestation report, as the firmware writes it (the SEV
//! Secure Nested Paging Firmware ABI, `ATTESTATION_REPORT`): 1,184 bytes,
//! of which the first 672 are signed by the chip's endorsement key with
//! ECDSA P-384 and SHA-384.

use crate::Error;

/// The size of a report.
pub const REPORT_SIZE: usize = 0x4A0;

/// The bytes the signature covers: those before it.
pub const SIGNED_SIZE: usize = 0x2A0;

/// The signature algorithm a report names for ECDSA P-384 with SHA-384.
pub const ECDSA_P384_SHA384: u32 = 1;

/// The guest policy's bit that lets the host debug the guest, reading its
/// memory: a guest launched with it proves nothing about what it keeps.
pub const POLICY_DEBUG: u64 = 1 << 19;

/// Which key signed a report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigningKey {
    /// The chip's own endorsement key, derived for its firmware's version.
    Vcek,
    /// A key AMD issued to a cloud provider for its chips.
    Vlek,
    /// Another value (none, or reserved).
    Other(u8),
}

/// A report, kept as its bytes, read field by field.
#[derive(Clone, PartialEq, Eq)]
pub struct Report {
    bytes: Vec<u8>,
}

impl std::fmt::Debug for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Report")
            .field("version", &self.version())
            .field("measurement", &hex::encode(self.measurement()))
            .field("report_data", &hex::encode(self.report_data()))
            .finish_non_exhaustive()
    }
}

fn le_u32(bytes: &[u8], at: usize) -> u32 {
    let mut word = [0u8; 4];
    word.copy_from_slice(&bytes[at..at + 4]);
    u32::from_le_bytes(word)
}

fn le_u64(bytes: &[u8], at: usize) -> u64 {
    let mut word = [0u8; 8];
    word.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(word)
}

impl Report {
    /// Reads a report from its bytes: exactly [`REPORT_SIZE`] of them, of
    /// version 2 or later.
    pub fn parse(bytes: &[u8]) -> Result<Report, Error> {
        if bytes.len() != REPORT_SIZE {
            return Err(Error::Malformed(format!(
                "a report is {REPORT_SIZE} bytes, not {}",
                bytes.len()
            )));
        }
        let report = Report {
            bytes: bytes.to_vec(),
        };
        if report.version() < 2 {
            return Err(Error::Malformed(format!(
                "a report of version {} is older than this reads",
                report.version()
            )));
        }
        Ok(report)
    }

    /// Its bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The bytes its signature covers.
    pub fn signed(&self) -> &[u8] {
        &self.bytes[..SIGNED_SIZE]
    }

    /// Its format's version.
    pub fn version(&self) -> u32 {
        le_u32(&self.bytes, 0x00)
    }

    /// The guest's security version.
    pub fn guest_svn(&self) -> u32 {
        le_u32(&self.bytes, 0x04)
    }

    /// The policy the guest was launched with.
    pub fn policy(&self) -> u64 {
        le_u64(&self.bytes, 0x08)
    }

    /// Whether its policy lets the host debug the guest.
    pub fn debug_allowed(&self) -> bool {
        self.policy() & POLICY_DEBUG != 0
    }

    /// The privilege level of whoever asked for the report.
    pub fn vmpl(&self) -> u32 {
        le_u32(&self.bytes, 0x30)
    }

    /// The signature algorithm it names.
    pub fn signature_algo(&self) -> u32 {
        le_u32(&self.bytes, 0x34)
    }

    /// Which key signed it.
    pub fn signing_key(&self) -> SigningKey {
        match (le_u32(&self.bytes, 0x48) >> 2) & 0b111 {
            0 => SigningKey::Vcek,
            1 => SigningKey::Vlek,
            other => SigningKey::Other(other as u8),
        }
    }

    /// Whether its chip identifier was left out (masked to zeros).
    pub fn chip_id_masked(&self) -> bool {
        le_u32(&self.bytes, 0x48) & 0b10 != 0
    }

    /// The 64 bytes the guest asked to be bound in it.
    pub fn report_data(&self) -> [u8; 64] {
        let mut data = [0u8; 64];
        data.copy_from_slice(&self.bytes[0x50..0x90]);
        data
    }

    /// The measurement of what the guest was launched with.
    pub fn measurement(&self) -> [u8; 48] {
        let mut measurement = [0u8; 48];
        measurement.copy_from_slice(&self.bytes[0x90..0xC0]);
        measurement
    }

    /// Data the host gave the guest at launch.
    pub fn host_data(&self) -> [u8; 32] {
        let mut data = [0u8; 32];
        data.copy_from_slice(&self.bytes[0xC0..0xE0]);
        data
    }

    /// The version of the firmware and microcode the report was made with,
    /// as its endorsement key was derived for.
    pub fn reported_tcb(&self) -> u64 {
        le_u64(&self.bytes, 0x180)
    }

    /// The chip's identifier.
    pub fn chip_id(&self) -> [u8; 64] {
        let mut id = [0u8; 64];
        id.copy_from_slice(&self.bytes[0x1A0..0x1E0]);
        id
    }

    /// Its signature as the fixed `r ‖ s` that ECDSA checks take: each 48
    /// bytes, big-endian. The report keeps each in 72 bytes, little-endian;
    /// a value whose upper bytes are not zero is not a P-384 one.
    pub fn signature_fixed(&self) -> Result<[u8; 96], Error> {
        let mut fixed = [0u8; 96];
        for (part, at) in [(0usize, SIGNED_SIZE), (1, SIGNED_SIZE + 72)] {
            let field = &self.bytes[at..at + 72];
            if field[48..].iter().any(|b| *b != 0) {
                return Err(Error::Malformed(
                    "the report's signature is not a P-384 one".into(),
                ));
            }
            for (i, byte) in field[..48].iter().rev().enumerate() {
                fixed[part * 48 + i] = *byte;
            }
        }
        Ok(fixed)
    }
}

/// Writes a report of version 2 with these fields, unsigned: for a
/// simulated platform ([`crate::simulated`]) and for tests.
pub fn compose(
    report_data: &[u8; 64],
    measurement: &[u8; 48],
    policy: u64,
    chip_id: &[u8; 64],
) -> Vec<u8> {
    let mut bytes = vec![0u8; REPORT_SIZE];
    bytes[0x00..0x04].copy_from_slice(&2u32.to_le_bytes());
    bytes[0x08..0x10].copy_from_slice(&policy.to_le_bytes());
    bytes[0x34..0x38].copy_from_slice(&ECDSA_P384_SHA384.to_le_bytes());
    bytes[0x50..0x90].copy_from_slice(report_data);
    bytes[0x90..0xC0].copy_from_slice(measurement);
    bytes[0x1A0..0x1E0].copy_from_slice(chip_id);
    bytes
}

/// Puts a fixed `r ‖ s` signature (each 48 bytes, big-endian) into
/// `report`, as the firmware keeps it.
pub fn put_signature(report: &mut [u8], fixed: &[u8]) {
    for (part, at) in [(0usize, SIGNED_SIZE), (1, SIGNED_SIZE + 72)] {
        let field = &mut report[at..at + 72];
        field.fill(0);
        for (i, byte) in fixed[part * 48..part * 48 + 48].iter().rev().enumerate() {
            field[i] = *byte;
        }
    }
}
