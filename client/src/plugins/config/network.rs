use serde::Deserialize;

use crate::plugins::config::division::DivisionInfo;

#[derive(Deserialize, Debug)]
pub struct NetworkSettings {
    pub enabled: bool,
    /// Overrides the gateway from the user's own `Media.pk2`
    /// (`divisioninfo.txt` + `gateport.txt`). Omit it to use that data —
    /// set it to point at a local stub or another server.
    #[serde(default)]
    pub gateway_address: Option<String>,
    /// Append every received packet payload to `packet_dump/<opcode>.log`,
    /// and every sent one to `packet_dump/c2s/<opcode>.log` (one hex line per
    /// packet) for offline re-analysis.
    #[serde(default = "default_true")]
    pub packet_dump: bool,
    /// Whether to send item-use (0x704C) requests. Off by default: the
    /// maintainer's go-sro build does not implement 0x704C and resets the
    /// connection on receipt (#215). Enable only against a server that
    /// supports it.
    #[serde(default)]
    pub item_use_enabled: bool,
    /// Apply the original client's outbound Blowfish policy: encrypt the
    /// identity/login opcodes (`ENCRYPTED_SEND_OPCODES`), leave everything
    /// else cleartext. Off by default: without it we send the account password
    /// in plaintext (#243), but the maintainer's server accepts our plaintext
    /// login today and it is unverified that it accepts an encrypted one
    /// (`docs/re/net/encryption-policy.md` UNKNOWN-1). Turn on once one live
    /// login confirms it.
    #[serde(default)]
    pub outbound_encryption: bool,
}

impl NetworkSettings {
    /// The address to connect to: `config.yaml` when it names one, otherwise
    /// the gateway the user's own PK2 data points at. `None` means neither is
    /// available, which the caller reports rather than guessing a host.
    pub fn resolve_gateway(&self, division: &DivisionInfo) -> Option<String> {
        self.gateway_address
            .clone()
            .or_else(|| division.gateway_address())
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::config::division::Division;

    fn settings(gateway_address: Option<&str>) -> NetworkSettings {
        NetworkSettings {
            enabled: true,
            gateway_address: gateway_address.map(str::to_string),
            packet_dump: false,
            item_use_enabled: false,
            outbound_encryption: false,
        }
    }

    fn pk2_division() -> DivisionInfo {
        DivisionInfo {
            content_id: 22,
            divisions: vec![Division {
                name: "DIV01".into(),
                gateways: vec!["filter.example.com".into()],
            }],
            gateway_port: Some(4001),
        }
    }

    #[test]
    fn config_overrides_the_pk2_gateway() {
        let resolved = settings(Some("127.0.0.1:15779")).resolve_gateway(&pk2_division());
        assert_eq!(resolved.as_deref(), Some("127.0.0.1:15779"));
    }

    #[test]
    fn pk2_supplies_the_gateway_when_config_is_silent() {
        let resolved = settings(None).resolve_gateway(&pk2_division());
        assert_eq!(resolved.as_deref(), Some("filter.example.com:4001"));
    }

    #[test]
    fn no_gateway_anywhere_resolves_to_none() {
        assert_eq!(
            settings(None).resolve_gateway(&DivisionInfo::default()),
            None
        );
    }
}
