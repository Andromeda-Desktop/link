//! Whether phones on the LAN reach linkd through the firewall, the D-Bus `LanState` property. The Windows doctor
//! (`win_net`) reads the firewall into [`Facts`]; elsewhere no doctor runs and the state stays `Unknown`.

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(not(any(windows, test)), expect(dead_code, reason = "only the Windows doctor finds a state"))]
pub enum LanState {
    /// No doctor, a firewall managed by Group Policy, or the firewall cannot be read.
    #[default]
    Unknown,
    Ok,
    /// The firewall of a Domain or Private network blocks a port: linkd's allow rule is missing or a block rule
    /// overrides it.
    RuleMissing,
    /// Only a Public network blocks a port; the installer opens them on Private and Domain networks only.
    PublicNetwork,
    /// No connected network on a hardware adapter.
    NoInterface,
}

impl LanState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Ok => "ok",
            Self::RuleMissing => "rule-missing",
            Self::PublicNetwork => "public-network",
            Self::NoInterface => "no-interface",
        }
    }
}

/// `NET_FW_PROFILE_TYPE2` bits.
#[cfg(any(windows, test))]
pub const DOMAIN: u32 = 1;
#[cfg(any(windows, test))]
pub const PRIVATE: u32 = 2;
#[cfg(any(windows, test))]
pub const PUBLIC: u32 = 4;

/// The firewall profile of a connected network.
#[cfg(any(windows, test))]
#[derive(Debug, Clone)]
pub struct Profile {
    /// One of [`DOMAIN`], [`PRIVATE`], [`PUBLIC`].
    pub bit: u32,
    pub firewall_on: bool,
    pub blocks_by_default: bool,
    /// "Block all incoming connections", which ignores allow rules.
    pub blocks_all: bool,
}

/// An enabled inbound rule for linkd's program or for every program.
#[cfg(any(windows, test))]
#[derive(Debug, Clone)]
pub struct Rule {
    pub allow: bool,
    /// `NET_FW_PROFILE_TYPE2` mask.
    pub profiles: u32,
    /// UDP or any protocol.
    pub udp: bool,
    /// `INetFwRule::LocalPorts`: `*`, empty, or a comma list of ports, ranges and keywords.
    pub ports: String,
    /// Limited to some remote addresses (other than the local subnet), interfaces or users: whether it matches a
    /// phone is not known, so it neither opens nor blocks.
    pub scoped: bool,
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone)]
pub struct Facts {
    /// Group Policy overrides local rules or blocks inbound traffic on a current profile.
    pub managed: bool,
    /// One per profile of the connected networks on hardware adapters (no Hyper-V, WSL or VPN adapters).
    pub profiles: Vec<Profile>,
    pub rules: Vec<Rule>,
}

/// The state for the UDP `ports` under Windows Firewall precedence: a matching block rule wins over any allow rule,
/// an allow rule wins over a blocking default.
#[cfg(any(windows, test))]
pub fn classify(facts: &Facts, ports: &[u16]) -> LanState {
    if facts.managed {
        return LanState::Unknown;
    }
    if facts.profiles.is_empty() {
        return LanState::NoInterface;
    }
    let blocks = |profile: &Profile, port: u16| {
        let mut matching = facts
            .rules
            .iter()
            .filter(|rule| !rule.scoped && rule.profiles & profile.bit != 0 && rule.udp && covers(&rule.ports, port));
        profile.firewall_on
            && (profile.blocks_all
                || matching.clone().any(|rule| !rule.allow)
                || (profile.blocks_by_default && !matching.any(|rule| rule.allow)))
    };
    let blocked: u32 = facts
        .profiles
        .iter()
        .filter(|profile| ports.iter().any(|&port| blocks(profile, port)))
        .map(|profile| profile.bit)
        .fold(0, |mask, bit| mask | bit);
    if blocked & (DOMAIN | PRIVATE) != 0 {
        LanState::RuleMissing
    } else if blocked & PUBLIC != 0 {
        LanState::PublicNetwork
    } else {
        LanState::Ok
    }
}

#[cfg(any(windows, test))]
fn covers(ports: &str, port: u16) -> bool {
    let ports = ports.trim();
    ports.is_empty()
        || ports == "*"
        || ports.split(',').map(str::trim).any(|item| match item.split_once('-') {
            Some((low, high)) => {
                low.trim().parse::<u16>().is_ok_and(|low| low <= port)
                    && high.trim().parse::<u16>().is_ok_and(|high| port <= high)
            }
            None => item.parse::<u16>() == Ok(port),
        })
}

#[cfg(test)]
mod tests {
    use super::{DOMAIN, Facts, LanState, PRIVATE, PUBLIC, Profile, Rule, classify};

    const QUIC: u16 = 4717;
    const MDNS: u16 = 5353;

    fn profile(bit: u32) -> Profile {
        Profile { bit, firewall_on: true, blocks_by_default: true, blocks_all: false }
    }

    fn allow(profiles: u32, ports: &str) -> Rule {
        Rule { allow: true, profiles, udp: true, ports: ports.to_owned(), scoped: false }
    }

    fn state(profiles: &[Profile], rules: &[Rule]) -> LanState {
        let facts = Facts { managed: false, profiles: profiles.to_vec(), rules: rules.to_vec() };
        classify(&facts, &[QUIC, MDNS])
    }

    #[test]
    fn the_installer_rule_opens_private_and_domain_but_not_public() {
        let installer = allow(DOMAIN | PRIVATE, "*");
        assert_eq!(state(&[profile(PRIVATE)], std::slice::from_ref(&installer)), LanState::Ok);
        assert_eq!(state(&[profile(DOMAIN)], std::slice::from_ref(&installer)), LanState::Ok);
        assert_eq!(state(&[profile(PUBLIC)], std::slice::from_ref(&installer)), LanState::PublicNetwork);
        assert_eq!(state(&[profile(PRIVATE), profile(PUBLIC)], &[installer]), LanState::PublicNetwork);
        assert_eq!(state(&[profile(PRIVATE)], &[]), LanState::RuleMissing);
        assert_eq!(state(&[profile(PRIVATE), profile(PUBLIC)], &[]), LanState::RuleMissing);
    }

    #[test]
    fn every_port_must_be_open() {
        assert_eq!(state(&[profile(PRIVATE)], &[allow(PRIVATE, "4717")]), LanState::RuleMissing);
        assert_eq!(state(&[profile(PRIVATE)], &[allow(PRIVATE, "4717, 5353")]), LanState::Ok);
        assert_eq!(state(&[profile(PRIVATE)], &[allow(PRIVATE, "4700-4720,5353")]), LanState::Ok);
        assert_eq!(state(&[profile(PRIVATE)], &[allow(PRIVATE, "4718-4720,5353,RPC")]), LanState::RuleMissing);
        let tcp = Rule { udp: false, ..allow(PRIVATE, "*") };
        assert_eq!(state(&[profile(PRIVATE)], &[tcp]), LanState::RuleMissing);
    }

    #[test]
    fn a_block_rule_or_block_all_wins_over_the_allow_rule() {
        let installer = allow(DOMAIN | PRIVATE, "*");
        let mdns_block = Rule { allow: false, ..allow(PRIVATE, "5353") };
        assert_eq!(state(&[profile(PRIVATE)], &[installer.clone(), mdns_block.clone()]), LanState::RuleMissing);
        assert_eq!(state(&[profile(DOMAIN)], &[installer.clone(), mdns_block]), LanState::Ok);
        let shields_up = Profile { blocks_all: true, ..profile(PRIVATE) };
        assert_eq!(state(&[shields_up], &[installer]), LanState::RuleMissing);
    }

    #[test]
    fn a_scoped_rule_neither_opens_nor_blocks() {
        let installer = allow(DOMAIN | PRIVATE, "*");
        let one_address_block = Rule { allow: false, scoped: true, ..allow(PRIVATE, "*") };
        assert_eq!(state(&[profile(PRIVATE)], &[installer, one_address_block]), LanState::Ok);
        let one_address_allow = Rule { scoped: true, ..allow(PRIVATE, "*") };
        assert_eq!(state(&[profile(PRIVATE)], &[one_address_allow]), LanState::RuleMissing);
    }

    #[test]
    fn an_open_firewall_needs_no_rule_and_policy_or_no_network_comes_first() {
        let off = Profile { firewall_on: false, ..profile(PUBLIC) };
        assert_eq!(state(&[off], &[]), LanState::Ok);
        let allows = Profile { blocks_by_default: false, ..profile(PRIVATE) };
        assert_eq!(state(&[allows], &[]), LanState::Ok);
        assert_eq!(state(&[], &[]), LanState::NoInterface);
        let managed = Facts { managed: true, profiles: vec![profile(PRIVATE)], rules: vec![] };
        assert_eq!(classify(&managed, &[QUIC]), LanState::Unknown);
    }
}
