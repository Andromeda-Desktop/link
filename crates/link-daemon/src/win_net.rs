//! The network on Windows, watched without polling on one thread: each address change makes the mDNS responder
//! recheck (the Linux netlink watch's twin), and the firewall doctor sets `LanState` after network or firewall
//! changes. Changes are read once a burst has been quiet for [`QUIET`].

#![allow(unsafe_code, reason = "FFI: COM, registry, IP Helper and event calls on handles this module owns")]

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use std::{io, ptr};

use anyhow::Context as _;
use windows::Win32::NetworkManagement::WindowsFirewall::{
    INetFwPolicy2, INetFwRule, INetFwRule3, NET_FW_ACTION_ALLOW, NET_FW_ACTION_BLOCK, NET_FW_IP_PROTOCOL_ANY,
    NET_FW_IP_PROTOCOL_UDP, NET_FW_MODIFY_STATE_OK, NET_FW_PROFILE_TYPE2, NET_FW_RULE_DIR_IN, NetFwPolicy2,
};
use windows::Win32::Networking::NetworkListManager::{
    INetwork, INetworkEvents, INetworkListManager, NLM_ENUM_NETWORK_CONNECTED,
    NLM_NETWORK_CATEGORY_DOMAIN_AUTHENTICATED, NLM_NETWORK_CATEGORY_PRIVATE, NetworkListManager,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, IConnectionPoint, IConnectionPointContainer,
    IDispatch,
};
use windows::Win32::System::Ole::IEnumVARIANT;
use windows::Win32::System::Variant::VARIANT;
use windows::core::Interface as _;
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    CancelMibChangeNotify2, ConvertInterfaceGuidToLuid, GetIfEntry2, MIB_IF_ROW2, MIB_NOTIFICATION_TYPE,
    MIB_UNICASTIPADDRESS_ROW, NotifyUnicastIpAddressChange,
};
use windows_sys::Win32::Networking::WinSock::AF_UNSPEC;
use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_NOTIFY, REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_CHANGE_NAME, RegCloseKey,
    RegNotifyChangeKeyValue, RegOpenKeyExW,
};
use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects};

use crate::hub::HubHandle;
use crate::lan::{self, DOMAIN, Facts, LanState, PRIVATE, PUBLIC, Profile, Rule};

/// A burst of changes (a network switch, the firewall writing a rule's values) settles within this, in ms.
const QUIET: u32 = 1000;
/// A stream of changes delays the read at most this long.
const LONGEST_WAIT: Duration = Duration::from_secs(5);
const MDNS: u16 = 5353;
/// Where the firewall service keeps its profiles and rules.
const FIREWALL_POLICY: &str = r"SYSTEM\CurrentControlSet\Services\SharedAccess\Parameters\FirewallPolicy";

/// Watches until the process ends; `port` is linkd's bound QUIC port.
pub fn start(hub: HubHandle, mdns: Option<mdns_sd::ServiceDaemon>, port: u16) {
    let runtime = tokio::runtime::Handle::current();
    let spawned = std::thread::Builder::new().name("win-net".to_owned()).spawn(move || {
        if let Err(error) = run(&runtime, &hub, mdns.as_ref(), port) {
            log::warn!("network watch stopped; LanState stays unknown and mDNS follows no changes: {error:#}");
        }
    });
    if let Err(error) = spawned {
        log::warn!("cannot start the network watch: {error}");
    }
}

fn run(
    runtime: &tokio::runtime::Handle,
    hub: &HubHandle,
    mdns: Option<&mdns_sd::ServiceDaemon>,
    port: u16,
) -> anyhow::Result<()> {
    let program = std::fs::canonicalize(std::env::current_exe()?).context("finding andromeda-linkd.exe")?;
    // SAFETY: once per thread, before any COM call on it.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
    let wake = Arc::new(Wake { event: Event::new()?, address: AtomicBool::new(false) });
    let _addresses = AddressWatch::new(&wake)
        .inspect_err(|error| log::warn!("mDNS sees address changes within a minute: {error}"))
        .ok();
    // SAFETY: COM is initialized on this thread.
    let networks: INetworkListManager = unsafe { CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL)? };
    let _network_watch = NetworkWatch::new(&networks, &wake)
        .inspect_err(|error| log::warn!("network changes show at the next address or firewall change: {error}"))
        .ok();
    let mut firewall = FirewallWatch::new()
        .inspect_err(|error| log::warn!("firewall rule changes show at the next network change: {error}"))
        .ok();
    let mut last = None;
    loop {
        let state = facts(&networks, &program).map_or_else(
            |error| {
                log::warn!("cannot read the firewall: {error:#}");
                LanState::Unknown
            },
            |facts| lan::classify(&facts, &[port, MDNS]),
        );
        if last != Some(state) {
            log::info!("LAN state: {}", state.as_str());
            runtime.block_on(hub.set_lan(state));
            last = Some(state);
        }
        let (mut timeout, mut deadline) = (INFINITE, None);
        loop {
            let handles: Vec<HANDLE> =
                [Some(wake.event.0), firewall.as_ref().map(|watch| watch.event.0)].into_iter().flatten().collect();
            // SAFETY: every handle stays open for this wait.
            let signalled =
                unsafe { WaitForMultipleObjects(u32::try_from(handles.len())?, handles.as_ptr(), 0, timeout) };
            if signalled == WAIT_TIMEOUT {
                break;
            }
            if signalled == WAIT_OBJECT_0 + 1 {
                if let Some(Err(error)) = firewall.as_ref().map(FirewallWatch::arm) {
                    log::warn!("firewall rule changes show at the next network change: {error}");
                    firewall = None;
                }
            } else if signalled != WAIT_OBJECT_0 {
                anyhow::bail!("waiting for network changes: {}", io::Error::last_os_error());
            }
            if wake.address.swap(false, Ordering::Relaxed)
                && let Some(mdns) = mdns
                && let Err(error) = link_core::discovery::recheck_interfaces(mdns)
            {
                log::warn!("mdns: {error}");
            }
            let left =
                deadline.get_or_insert_with(|| Instant::now() + LONGEST_WAIT).saturating_duration_since(Instant::now());
            timeout = u32::try_from(left.as_millis()).unwrap_or(u32::MAX).min(QUIET);
        }
    }
}

/// The firewall's view for the connected networks on hardware adapters, with the enabled inbound rules that apply to
/// `program` or to every program (no service or app package rules).
fn facts(networks: &INetworkListManager, program: &Path) -> anyhow::Result<Facts> {
    // SAFETY: COM calls on interfaces owned here, on this thread's MTA; each VARIANT is cleared by its Drop.
    unsafe {
        let policy: INetFwPolicy2 = CoCreateInstance(&NetFwPolicy2, None, CLSCTX_ALL)?;
        let managed = policy.LocalPolicyModifyState()? != NET_FW_MODIFY_STATE_OK;
        // Each adapter's network has its own profile (Windows Firewall, multiple active profiles).
        let mut active = 0;
        let list = networks.GetNetworks(NLM_ENUM_NETWORK_CONNECTED)?;
        loop {
            let mut item = [None];
            list.Next(&mut item, None)?;
            let Some(network) = item[0].take() else { break };
            if on_hardware(&network)? {
                active |= match network.GetCategory()? {
                    NLM_NETWORK_CATEGORY_DOMAIN_AUTHENTICATED => DOMAIN,
                    NLM_NETWORK_CATEGORY_PRIVATE => PRIVATE,
                    _ => PUBLIC,
                };
            }
        }
        let mut profiles = Vec::new();
        for bit in [DOMAIN, PRIVATE, PUBLIC] {
            if active & bit == 0 {
                continue;
            }
            let kind = NET_FW_PROFILE_TYPE2(bit.cast_signed());
            profiles.push(Profile {
                bit,
                firewall_on: policy.get_FirewallEnabled(kind)?.as_bool(),
                blocks_by_default: policy.get_DefaultInboundAction(kind)? == NET_FW_ACTION_BLOCK,
                blocks_all: policy.get_BlockAllInboundTraffic(kind)?.as_bool(),
            });
        }
        let rules: IEnumVARIANT = policy.Rules()?._NewEnum()?.cast()?;
        let mut kept = Vec::new();
        loop {
            let mut item = [VARIANT::default()];
            let mut fetched = 0;
            rules.Next(&mut item, &raw mut fetched).ok()?;
            if fetched == 0 {
                break;
            }
            let rule: INetFwRule = IDispatch::try_from(&item[0])?.cast()?;
            if !rule.Enabled()?.as_bool() || rule.Direction()? != NET_FW_RULE_DIR_IN || !rule.ServiceName()?.is_empty()
            {
                continue;
            }
            let path = rule.ApplicationName()?;
            if !path.is_empty() && !same_program(&expand(&path.to_string()), program) {
                continue;
            }
            let mut for_users = false;
            if let Ok(rule) = rule.cast::<INetFwRule3>() {
                if !rule.LocalAppPackageId()?.is_empty() {
                    continue;
                }
                for_users = !rule.LocalUserAuthorizedList()?.is_empty();
            }
            let remote = rule.RemoteAddresses()?.to_string();
            let protocol = rule.Protocol()?;
            kept.push(Rule {
                allow: rule.Action()? == NET_FW_ACTION_ALLOW,
                profiles: rule.Profiles()?.cast_unsigned(),
                udp: protocol == NET_FW_IP_PROTOCOL_UDP.0 || protocol == NET_FW_IP_PROTOCOL_ANY.0,
                ports: rule.LocalPorts()?.to_string(),
                scoped: for_users
                    || !(remote == "*" || remote.eq_ignore_ascii_case("LocalSubnet"))
                    || !rule.InterfaceTypes()?.to_string().eq_ignore_ascii_case("All")
                    || !rule.Interfaces()?.is_empty(),
            });
        }
        Ok(Facts { managed, profiles, rules: kept })
    }
}

/// A connection of `network` runs on a hardware adapter, not on a Hyper-V, WSL or VPN one.
fn on_hardware(network: &INetwork) -> windows::core::Result<bool> {
    // SAFETY: COM calls on interfaces owned here; the row is plain data the call fills.
    unsafe {
        let connections = network.GetNetworkConnections()?;
        loop {
            let mut item = [None];
            connections.Next(&mut item, None)?;
            let Some(connection) = item[0].take() else { return Ok(false) };
            let guid = windows_sys::core::GUID::from_u128(connection.GetAdapterId()?.to_u128());
            let mut row: MIB_IF_ROW2 = std::mem::zeroed();
            if ConvertInterfaceGuidToLuid(&raw const guid, &raw mut row.InterfaceLuid) == ERROR_SUCCESS
                && GetIfEntry2(&raw mut row) == ERROR_SUCCESS
                // HardwareInterface, the first flag bit (netioapi.h).
                && row.InterfaceAndOperStatusFlags._bitfield & 1 != 0
            {
                return Ok(true);
            }
        }
    }
}

/// `path` names `program` (canonical), also through a short name, a link or other letter case.
fn same_program(path: &str, program: &Path) -> bool {
    let path = Path::new(path);
    path.file_name().zip(program.file_name()).is_some_and(|(rule, own)| rule.eq_ignore_ascii_case(own))
        && std::fs::canonicalize(path).is_ok_and(|path| path == program)
}

/// `path` with its `%VARIABLE%`s expanded, as rules may name programs under `%ProgramFiles%`.
fn expand(path: &str) -> String {
    let source: Vec<u16> = path.encode_utf16().chain([0]).collect();
    let mut expanded = vec![0u16; 512];
    loop {
        let capacity = u32::try_from(expanded.len()).unwrap_or(u32::MAX);
        // SAFETY: source is NUL-terminated; expanded holds `capacity` units.
        let needed = unsafe { ExpandEnvironmentStringsW(source.as_ptr(), expanded.as_mut_ptr(), capacity) };
        match needed {
            0 => return path.to_owned(),
            needed if needed <= capacity => {
                return String::from_utf16_lossy(&expanded[..needed as usize - 1]);
            }
            needed => expanded.resize(needed as usize, 0),
        }
    }
}

/// An auto-reset event.
struct Event(HANDLE);

// SAFETY: an event handle may be set and waited on from any thread.
unsafe impl Send for Event {}
// SAFETY: as above; SetEvent and waits are thread-safe.
unsafe impl Sync for Event {}

impl Event {
    fn new() -> io::Result<Self> {
        // SAFETY: an unnamed auto-reset event, not signalled, default security.
        let handle = unsafe { CreateEventW(ptr::null(), 0, 0, ptr::null()) };
        if handle.is_null() { Err(io::Error::last_os_error()) } else { Ok(Self(handle)) }
    }

    fn set(&self) {
        // SAFETY: the handle is open while self lives.
        unsafe { SetEvent(self.0) };
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: created in `new`, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// What the callbacks, which run on system threads, tell the watch thread.
struct Wake {
    event: Event,
    /// An address changed since the last mDNS recheck.
    address: AtomicBool,
}

/// `NotifyUnicastIpAddressChange`, cancelled on drop before its context goes.
struct AddressWatch {
    handle: HANDLE,
    _wake: Arc<Wake>,
}

impl AddressWatch {
    fn new(wake: &Arc<Wake>) -> io::Result<Self> {
        unsafe extern "system" fn changed(
            context: *const core::ffi::c_void,
            _row: *const MIB_UNICASTIPADDRESS_ROW,
            _kind: MIB_NOTIFICATION_TYPE,
        ) {
            // SAFETY: context is the Arc<Wake> that AddressWatch keeps alive until the registration is cancelled.
            let wake = unsafe { &*context.cast::<Wake>() };
            wake.address.store(true, Ordering::Relaxed);
            wake.event.set();
        }
        let mut handle = ptr::null_mut();
        // SAFETY: the context outlives the registration (cancelled in Drop, which runs on this thread, not in the
        // callback).
        let status = unsafe {
            NotifyUnicastIpAddressChange(AF_UNSPEC, Some(changed), Arc::as_ptr(wake).cast(), false, &raw mut handle)
        };
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status.cast_signed()));
        }
        Ok(Self { handle, _wake: Arc::clone(wake) })
    }
}

impl Drop for AddressWatch {
    fn drop(&mut self) {
        // SAFETY: registered in `new`; not called from the callback's thread.
        unsafe { CancelMibChangeNotify2(self.handle) };
    }
}

/// `INetworkEvents` on the Network List Manager: networks come, go, change connectivity or category.
struct NetworkWatch {
    point: IConnectionPoint,
    cookie: u32,
}

impl NetworkWatch {
    fn new(networks: &INetworkListManager, wake: &Arc<Wake>) -> windows::core::Result<Self> {
        let sink: INetworkEvents = sink::Changes(Arc::clone(wake)).into();
        // SAFETY: COM calls on this thread's MTA; the sink stays advised until Drop.
        unsafe {
            let point = networks.cast::<IConnectionPointContainer>()?.FindConnectionPoint(&INetworkEvents::IID)?;
            let cookie = point.Advise(&sink)?;
            Ok(Self { point, cookie })
        }
    }
}

impl Drop for NetworkWatch {
    fn drop(&mut self) {
        // SAFETY: advised in `new`.
        drop(unsafe { self.point.Unadvise(self.cookie) });
    }
}

/// A change notification on the firewall's registry key, re-armed after each one.
struct FirewallWatch {
    key: HKEY,
    event: Event,
}

impl FirewallWatch {
    fn new() -> io::Result<Self> {
        let path: Vec<u16> = FIREWALL_POLICY.encode_utf16().chain([0]).collect();
        let mut key = ptr::null_mut();
        // SAFETY: path is NUL-terminated; key receives an open handle on success.
        let status = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, KEY_NOTIFY, &raw mut key) };
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status.cast_signed()));
        }
        let watch = Self { key, event: Event::new()? };
        watch.arm()?;
        Ok(watch)
    }

    /// Asks for one notification of a value or subkey change anywhere below the key.
    fn arm(&self) -> io::Result<()> {
        // SAFETY: key and event are open while self lives.
        let status = unsafe {
            RegNotifyChangeKeyValue(self.key, 1, REG_NOTIFY_CHANGE_NAME | REG_NOTIFY_CHANGE_LAST_SET, self.event.0, 1)
        };
        if status == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(status.cast_signed())) }
    }
}

impl Drop for FirewallWatch {
    fn drop(&mut self) {
        // SAFETY: opened in `new`, closed once; closing it ends the pending notification.
        unsafe { RegCloseKey(self.key) };
    }
}

/// The COM class; its `#[implement]` expansion trips these lints.
#[allow(clippy::inline_always, clippy::ref_as_ptr, reason = "code generated by windows-rs")]
mod sink {
    use std::sync::Arc;

    use windows::Win32::Networking::NetworkListManager::{
        INetworkEvents, INetworkEvents_Impl, NLM_CONNECTIVITY, NLM_NETWORK_PROPERTY_CHANGE,
    };
    use windows::core::{GUID, implement};

    use super::Wake;

    /// Called on RPC threads; every call wakes the watch thread, which rereads everything.
    #[implement(INetworkEvents)]
    pub(super) struct Changes(pub(super) Arc<Wake>);

    impl INetworkEvents_Impl for Changes_Impl {
        fn NetworkAdded(&self, _id: &GUID) -> windows::core::Result<()> {
            self.0.event.set();
            Ok(())
        }

        fn NetworkDeleted(&self, _id: &GUID) -> windows::core::Result<()> {
            self.0.event.set();
            Ok(())
        }

        fn NetworkConnectivityChanged(&self, _id: &GUID, _now: NLM_CONNECTIVITY) -> windows::core::Result<()> {
            self.0.event.set();
            Ok(())
        }

        fn NetworkPropertyChanged(&self, _id: &GUID, _what: NLM_NETWORK_PROPERTY_CHANGE) -> windows::core::Result<()> {
            self.0.event.set();
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Context as _;
    use windows::Win32::NetworkManagement::WindowsFirewall::{
        INetFwPolicy2, INetFwRule, NET_FW_ACTION_BLOCK, NET_FW_RULE_DIR_IN, NetFwPolicy2, NetFwRule,
    };
    use windows::Win32::Networking::NetworkListManager::{INetworkListManager, NetworkListManager};
    use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
    use windows::core::BSTR;

    use crate::lan::PRIVATE;

    /// Adds block rules for this test program and for another one that differs only in its program, and reads only
    /// the first back; needs an elevated shell, as on CI.
    #[test]
    fn the_doctor_reads_only_the_rules_for_its_program() -> anyhow::Result<()> {
        let program = std::fs::canonicalize(std::env::current_exe()?)?;
        let names = [0, 1].map(|n| BSTR::from(format!("andromeda-link-test-{}-{n}", std::process::id())));
        let paths = [program.to_string_lossy().into_owned(), r"C:\Windows\System32\notepad.exe".to_owned()];
        // SAFETY: COM calls on this test thread's MTA; the rules are removed before the policy drops.
        let facts = unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            let policy: INetFwPolicy2 = CoCreateInstance(&NetFwPolicy2, None, CLSCTX_ALL)?;
            for (name, path) in names.iter().zip(&paths) {
                let rule: INetFwRule = CoCreateInstance(&NetFwRule, None, CLSCTX_ALL)?;
                rule.SetName(name)?;
                rule.SetApplicationName(&BSTR::from(path.as_str()))?;
                rule.SetProtocol(17)?;
                rule.SetLocalPorts(&BSTR::from("4717,4800-4810"))?;
                rule.SetDirection(NET_FW_RULE_DIR_IN)?;
                rule.SetAction(NET_FW_ACTION_BLOCK)?;
                rule.SetProfiles(PRIVATE.cast_signed())?;
                rule.SetEnabled(true.into())?;
                policy.Rules()?.Add(&rule).context("adding a firewall rule needs an elevated shell")?;
            }
            let networks: INetworkListManager = CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL)?;
            let facts = super::facts(&networks, &program);
            for name in &names {
                policy.Rules()?.Remove(name)?;
            }
            facts?
        };
        let ours: Vec<_> = facts.rules.iter().filter(|rule| rule.ports == "4717,4800-4810").collect();
        assert_eq!(ours.len(), 1, "{:?}", facts.rules);
        let rule = ours[0];
        assert!(!rule.allow && rule.udp && rule.profiles == PRIVATE && !rule.scoped, "{rule:?}");
        Ok(())
    }
}
