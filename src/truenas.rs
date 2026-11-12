//! TrueNAS integration boundary and preflight rules.
//!
//! Network transport is deliberately not faked here: the qualification host must prove secure
//! JSON-RPC negotiation and the separate snapshot/ZFS data path before production enablement.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConnectionConfig {
    pub websocket_url: String,
    pub credential_secret_ref: String,
    pub certificate_verification: bool,
    pub expected_certificate_sha256: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConnectionConfigError {
    #[error("TrueNAS control URL must use authenticated secure WebSocket (wss)")]
    InsecureScheme,
    #[error("TrueNAS control URL requires a host")]
    MissingHost,
    #[error("certificate verification cannot be disabled")]
    CertificateVerificationDisabled,
    #[error("credential secret reference is required")]
    MissingCredentialReference,
    #[error("invalid TrueNAS control URL")]
    InvalidUrl,
}

impl ConnectionConfig {
    /// Validate the immutable security properties of a TrueNAS connection.
    ///
    /// # Errors
    ///
    /// Returns a structured error when secure WebSocket transport, certificate verification, a
    /// host, or a secret reference is missing.
    pub fn validate(&self) -> Result<(), ConnectionConfigError> {
        let url =
            url::Url::parse(&self.websocket_url).map_err(|_| ConnectionConfigError::InvalidUrl)?;
        if url.scheme() != "wss" {
            return Err(ConnectionConfigError::InsecureScheme);
        }
        if url.host_str().is_none() {
            return Err(ConnectionConfigError::MissingHost);
        }
        if !self.certificate_verification {
            return Err(ConnectionConfigError::CertificateVerificationDisabled);
        }
        if self.credential_secret_ref.trim().is_empty() {
            return Err(ConnectionConfigError::MissingCredentialReference);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NegotiatedApiVersion {
    pub version: String,
    pub advertised: Vec<String>,
    pub supported: Vec<String>,
}

/// Choose the newest exact version common to the server advertisement and client support list.
#[must_use]
pub fn negotiate_api_version(
    advertised: &[String],
    supported: &[String],
) -> Option<NegotiatedApiVersion> {
    let mut common: Vec<&String> = advertised
        .iter()
        .filter(|item| supported.contains(item))
        .collect();
    common.sort();
    common.last().map(|version| NegotiatedApiVersion {
        version: (*version).clone(),
        advertised: advertised.to_vec(),
        supported: supported.to_vec(),
    })
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeState {
    Passed,
    Blocked,
    NotRun,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DataPathProbe {
    pub snapshot_visible: ProbeState,
    pub file_read: ProbeState,
    pub zfs_send: ProbeState,
    pub zfs_receive: ProbeState,
    pub host_helper_version: Option<String>,
}

impl DataPathProbe {
    #[must_use]
    pub fn native_backup_ready(&self) -> bool {
        self.snapshot_visible == ProbeState::Passed && self.zfs_send == ProbeState::Passed
    }

    #[must_use]
    pub fn native_restore_ready(&self) -> bool {
        self.zfs_receive == ProbeState::Passed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secure_config() -> ConnectionConfig {
        ConnectionConfig {
            websocket_url: "wss://truenas.example/api/current".to_owned(),
            credential_secret_ref: "secret://truenas/runtime".to_owned(),
            certificate_verification: true,
            expected_certificate_sha256: Some("ab".repeat(32)),
        }
    }

    #[test]
    fn secure_connection_configuration_is_required() {
        assert_eq!(secure_config().validate(), Ok(()));
        let mut insecure = secure_config();
        insecure.websocket_url = "ws://truenas.example/api/current".to_owned();
        assert_eq!(
            insecure.validate(),
            Err(ConnectionConfigError::InsecureScheme)
        );
        let mut no_verify = secure_config();
        no_verify.certificate_verification = false;
        assert_eq!(
            no_verify.validate(),
            Err(ConnectionConfigError::CertificateVerificationDisabled)
        );
    }

    #[test]
    fn negotiation_requires_an_exact_supported_version() {
        let advertised = vec!["25.10.0".to_owned(), "26.04.0".to_owned()];
        let supported = vec!["25.10.0".to_owned(), "25.10.1".to_owned()];
        assert_eq!(
            negotiate_api_version(&advertised, &supported)
                .unwrap()
                .version,
            "25.10.0"
        );
        assert!(negotiate_api_version(&["26.04.0".to_owned()], &supported).is_none());
    }

    #[test]
    fn api_success_does_not_imply_a_working_native_data_path() {
        let probe = DataPathProbe {
            snapshot_visible: ProbeState::Passed,
            file_read: ProbeState::Passed,
            zfs_send: ProbeState::NotRun,
            zfs_receive: ProbeState::NotRun,
            host_helper_version: None,
        };
        assert!(!probe.native_backup_ready());
        assert!(!probe.native_restore_ready());
    }
}
