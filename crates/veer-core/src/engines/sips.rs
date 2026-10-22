//! macOS `sips` bridge for HEIC and AVIF, which have no permissive Rust
//! codecs. Only used when `/usr/bin/sips` exists.

use std::path::Path;
use std::process::Command;

use crate::catalog::Capabilities;
use crate::error::{Error, Result};

/// Converts `input` to `format` ("png", "heic", ...) at `out`.
pub fn convert(
    caps: &Capabilities,
    input: &Path,
    out: &Path,
    format: &str,
    quality: Option<u8>,
) -> Result<()> {
    let sips = caps
        .sips
        .as_ref()
        .ok_or_else(|| Error::Missing("HEIC and AVIF need macOS".into()))?;
    let mut cmd = Command::new(sips);
    cmd.arg("-s").arg("format").arg(format);
    if let Some(q) = quality {
        cmd.arg("-s").arg("formatOptions").arg(q.to_string());
    }
    cmd.arg(input).arg("--out").arg(out);
    let output = cmd.output()?;
    if !output.status.success() || !out.exists() {
        let msg = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Invalid(format!(
            "sips couldn't convert the file: {}",
            msg.trim()
        )));
    }
    Ok(())
}
