//! Data-availability sampling against a certified header.
//!
//! The certified header commits to the data root of the Pulse's extended
//! square. The light client fetches the square's axis roots (the
//! [`DaHeader`]), checks them against that root, then samples random shares;
//! with `s` samples a minimal withholding goes unnoticed with probability
//! below `(3/4)^s`.

use fold_da::{Availability, DaHeader, LightSampler, SampleReport, Share};

use crate::error::LightError;
use crate::header::PulseHeader;

/// Samples a Pulse's data. `fetch(row, column)` returns a share from any
/// source (`None` if it is not served).
pub fn sample<F>(
    header: &PulseHeader,
    da: &DaHeader,
    samples: usize,
    seed: u64,
    fetch: F,
) -> Result<SampleReport, LightError>
where
    F: FnMut(usize, usize) -> Option<Share>,
{
    if da.data_root != header.data_root || da.k != header.data_k {
        return Err(LightError::Availability(
            "data header does not match the certified data root".into(),
        ));
    }
    let report = LightSampler::new(samples, seed)
        .run(da, fetch)
        .map_err(|e| LightError::Availability(e.to_string()))?;
    match report.outcome {
        Availability::Available => Ok(report),
        Availability::Unavailable => Err(LightError::Availability(format!(
            "{} of {} samples were not served",
            report.missing,
            report.positions.len()
        ))),
        Availability::Invalid => Err(LightError::Availability(format!(
            "{} samples failed their proofs",
            report.invalid
        ))),
    }
}
