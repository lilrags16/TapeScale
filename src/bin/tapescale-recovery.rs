use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Parser)]
#[command(
    name = "tapescale-recovery",
    version,
    about = "Independent TapeScale recovery and evidence utility",
    long_about = "Inspects recovery bundles without the TapeScale database. This preview does not read physical tapes until an engine and format pass G0 qualification."
)]
struct Arguments {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Inspect and validate the non-secret manifest of a portable recovery bundle.
    InspectBundle { path: PathBuf },
    /// Print the independent clean-host recovery sequence.
    Runbook,
    /// Print the honest hardware/format qualification boundary.
    Qualification,
}

#[derive(Debug, Deserialize, Serialize)]
struct RecoveryBundleManifest {
    format: String,
    format_version: u32,
    created_at: String,
    installation_id: String,
    catalog_export: Artifact,
    media_map: Artifact,
    key_package_reference: String,
    required_software: Vec<SoftwareRequirement>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Artifact {
    file: String,
    sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct SoftwareRequirement {
    component: String,
    version: String,
}

fn main() -> Result<()> {
    match Arguments::parse().command {
        Command::InspectBundle { path } => inspect_bundle(&path),
        Command::Runbook => {
            println!(
                "1. Start from a clean qualified Linux host.\n\
                 2. Verify this CLI and engine packages against independently stored digests.\n\
                 3. Inspect the bundle; keep the key package separate until requested.\n\
                 4. Reconcile every physical tape identity without enabling overwrite.\n\
                 5. Import or rebuild the catalog through the selected engine's supported path.\n\
                 6. Resolve the complete dependency chain and approved restore destination.\n\
                 7. Restore into an isolated destination, verify, then perform an explicit cutover.\n\n\
                 See docs/RECOVERY.md. Physical recovery is NOT QUALIFIED in this alpha."
            );
            Ok(())
        }
        Command::Qualification => {
            println!(
                "{}",
                serde_json::to_string_pretty(&tapescale::qualification::report())?
            );
            Ok(())
        }
    }
}

fn inspect_bundle(path: &PathBuf) -> Result<()> {
    const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
    let metadata = fs::metadata(path).with_context(|| format!("cannot stat {}", path.display()))?;
    anyhow::ensure!(metadata.is_file(), "bundle manifest must be a regular file");
    anyhow::ensure!(
        metadata.len() <= MAX_MANIFEST_BYTES,
        "bundle manifest exceeds the 16 MiB safety limit"
    );
    let bytes = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let manifest: RecoveryBundleManifest =
        serde_json::from_slice(&bytes).context("invalid recovery bundle manifest")?;
    anyhow::ensure!(
        manifest.format == "tapescale-recovery-bundle",
        "unknown bundle format"
    );
    anyhow::ensure!(
        manifest.format_version == 1,
        "unsupported bundle format version"
    );
    validate_artifact(&manifest.catalog_export)?;
    validate_artifact(&manifest.media_map)?;
    anyhow::ensure!(
        !manifest.key_package_reference.trim().is_empty(),
        "key package reference is missing"
    );

    let digest = Sha256::digest(&bytes);
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "valid": true,
            "manifest_sha256": format!("{digest:x}"),
            "format_version": manifest.format_version,
            "created_at": manifest.created_at,
            "installation_id": manifest.installation_id,
            "catalog_export": manifest.catalog_export,
            "media_map": manifest.media_map,
            "key_package_reference": manifest.key_package_reference,
            "required_software": manifest.required_software,
            "notice": "Manifest inspection does not constitute a physical restore test."
        }))?
    );
    Ok(())
}

fn validate_artifact(artifact: &Artifact) -> Result<()> {
    anyhow::ensure!(!artifact.file.is_empty(), "artifact filename is empty");
    anyhow::ensure!(
        !artifact.file.starts_with('/') && !artifact.file.split('/').any(|part| part == ".."),
        "artifact filename escapes the recovery bundle"
    );
    anyhow::ensure!(
        artifact.sha256.len() == 64 && artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "artifact SHA-256 must contain exactly 64 hexadecimal characters"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_paths_cannot_escape_bundle() {
        let artifact = Artifact {
            file: "../catalog.sql".to_owned(),
            sha256: "0".repeat(64),
        };
        assert!(validate_artifact(&artifact).is_err());
    }
}
