//! The Linux kernel's configfs-tsm interface for attestation reports
//! (`/sys/kernel/config/tsm/report`, Linux 6.7 and later): a directory made
//! for one report, the 64 bytes to bind written to its `inblob`, the report
//! read from its `outblob`, and for SEV-SNP the certificates the host gives
//! (its table of them) from its `auxblob`. Files only, no device calls.

use std::path::{Path, PathBuf};

use crate::Error;

/// Where reports are asked for.
pub const REPORTS: &str = "/sys/kernel/config/tsm/report";

/// What the platform gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Collected {
    /// Its provider: `sev_guest` or `tdx_guest`.
    pub provider: String,
    /// The report (SEV-SNP) or quote (TDX).
    pub outblob: Vec<u8>,
    /// Certificates the host gave, if any.
    pub auxblob: Option<Vec<u8>>,
}

/// Asks for a report binding `data`, in `root` (normally [`REPORTS`]).
pub fn collect_in(root: &Path, data: &[u8; 64]) -> Result<Collected, Error> {
    let fail = |what: &str, path: &Path, e: std::io::Error| {
        Error::Collect(format!("{what} {}: {e}", path.display()))
    };
    if !root.is_dir() {
        return Err(Error::Unsupported(format!(
            "{} is not here: this is not a confidential VM, or its kernel has no configfs-tsm",
            root.display()
        )));
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let dir: PathBuf = root.join(format!("sion-{}-{nanos}", std::process::id()));
    std::fs::create_dir(&dir).map_err(|e| fail("making", &dir, e))?;
    let result = (|| {
        let inblob = dir.join("inblob");
        std::fs::write(&inblob, data).map_err(|e| fail("writing", &inblob, e))?;
        let read = |name: &str| {
            let path = dir.join(name);
            std::fs::read(&path).map_err(|e| fail("reading", &path, e))
        };
        let outblob = read("outblob")?;
        // A generation other than the one write: another process wrote to
        // this report meanwhile, and its report is not ours.
        let generation = String::from_utf8_lossy(&read("generation")?)
            .trim()
            .to_string();
        if generation != "1" {
            return Err(Error::Collect(format!(
                "the report was asked for again meanwhile (generation {generation})"
            )));
        }
        let provider = String::from_utf8_lossy(&read("provider")?)
            .trim()
            .to_string();
        let auxblob = read("auxblob").ok().filter(|blob| !blob.is_empty());
        Ok(Collected {
            provider,
            outblob,
            auxblob,
        })
    })();
    let _ = std::fs::remove_dir(&dir);
    result
}

/// Asks this machine's platform for a report binding `data`.
pub fn collect(data: &[u8; 64]) -> Result<Collected, Error> {
    collect_in(Path::new(REPORTS), data)
}

/// The VCEK's identifier in the host's table of certificates, as the GHCB
/// specification gives it.
const VCEK_GUID: &str = "63da758d-e664-4564-adc5-f4b93be8accd";

/// A GUID's 16 bytes as written (big-endian) and as UEFI keeps them (its
/// first three fields little-endian).
fn guid_forms(guid: &str) -> [[u8; 16]; 2] {
    let plain: Vec<u8> = hex::decode(guid.replace('-', "")).unwrap_or_default();
    let mut written = [0u8; 16];
    written.copy_from_slice(&plain[..16]);
    let mut uefi = written;
    uefi[0..4].reverse();
    uefi[4..6].reverse();
    uefi[6..8].reverse();
    [written, uefi]
}

/// The VCEK in a host's table of certificates: entries of a GUID, an
/// offset and a length (each four bytes, little-endian), ended by one all
/// zeros; the offsets from the table's start.
pub fn vcek_in(table: &[u8]) -> Option<Vec<u8>> {
    let forms = guid_forms(VCEK_GUID);
    for entry in table.chunks_exact(24) {
        let guid = &entry[..16];
        if guid.iter().all(|b| *b == 0) {
            break;
        }
        if forms.iter().any(|form| form == guid) {
            let offset = u32::from_le_bytes(entry[16..20].try_into().ok()?) as usize;
            let length = u32::from_le_bytes(entry[20..24].try_into().ok()?) as usize;
            return table
                .get(offset..offset.checked_add(length)?)
                .map(<[u8]>::to_vec);
        }
    }
    None
}
