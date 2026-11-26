//! The machines on this computer's tailnet, from the local `tailscale` command, offered as boxes
//! reached with the system's own `ssh` over the tailnet. Nothing here uses `tailscale ssh`.
use rex_rune::tailnet::{self, Machine};

/// A machine on the tailnet that a box could be added for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailnetMachine {
    /// Its first name on the tailnet, such as `forge`.
    pub name: String,
    /// Its MagicDNS name, or its Tailscale IP when the tailnet has none.
    pub address: String,
    /// `Linux`, `macOS`, …, or empty.
    pub os: String,
    pub online: bool,
    /// Tailscale SSH is on there. It answers port 22 on the tailnet address in place of `sshd`,
    /// so the tailnet's policy, not this computer's keys, decides who gets in.
    pub tailscale_ssh: bool,
    /// The box address: plain SSH to the tailnet address, as `ssh` itself would connect.
    pub endpoint: String,
}

impl TailnetMachine {
    fn from(machine: Machine) -> Self {
        Self {
            endpoint: machine.ssh_endpoint("", ""),
            os: machine.os_name().to_owned(),
            name: machine.name,
            address: machine.address,
            online: machine.online,
            tailscale_ssh: machine.ssh,
        }
    }
}

/// The other machines a box could be added for: not this one, nor an iPhone or iPad, which run
/// no `sshd`. Online ones first, each group by name. Asks the local Tailscale off the UI thread.
pub async fn tailnet() -> Result<Vec<TailnetMachine>, String> {
    let (done, wait) = flume::bounded(1);
    std::thread::spawn(move || {
        let _ = done.send(tailnet::status());
    });
    let machines = wait.recv_async().await.map_err(|_| "Tailscale could not be asked".to_owned())??;
    Ok(choose(machines))
}

fn choose(machines: Vec<Machine>) -> Vec<TailnetMachine> {
    let mut out: Vec<TailnetMachine> = machines
        .into_iter()
        .filter(|m| !m.this_machine && !m.is_ios())
        .map(TailnetMachine::from)
        .collect();
    // Stable, so the core's order by name holds within each group.
    out.sort_by_key(|m| !m.online);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn other_machines_are_offered_as_plain_ssh_over_the_tailnet() {
        let json = r#"{
            "Self": {"HostName": "pop-os", "DNSName": "here.tail1234.ts.net.", "OS": "linux",
                     "Online": true, "TailscaleIPs": ["100.64.0.1"]},
            "Peer": {
                "a": {"HostName": "forge", "DNSName": "forge.tail1234.ts.net.", "OS": "linux",
                      "Online": true, "sshHostKeys": ["ssh-ed25519 AAAA"], "TailscaleIPs": ["100.64.0.2"]},
                "b": {"HostName": "Phone", "DNSName": "phone.tail1234.ts.net.", "OS": "iOS",
                      "Online": true, "TailscaleIPs": ["100.64.0.3"]},
                "c": {"HostName": "attic", "DNSName": "attic.tail1234.ts.net.", "OS": "macOS",
                      "Online": false, "TailscaleIPs": ["100.64.0.4"]},
                "d": {"HostName": "nas", "DNSName": "", "OS": "linux",
                      "Online": true, "TailscaleIPs": ["fd7a:115c:a1e0::5", "100.64.0.5"]}
            }
        }"#;
        let machines = choose(tailnet::parse_status(json).unwrap());
        let seen: Vec<(&str, &str, bool)> =
            machines.iter().map(|m| (m.name.as_str(), m.endpoint.as_str(), m.online)).collect();
        assert_eq!(
            seen,
            [
                ("forge", "ssh://forge.tail1234.ts.net", true),
                ("nas", "ssh://100.64.0.5", true),
                ("attic", "ssh://attic.tail1234.ts.net", false),
            ]
        );
        assert!(machines[0].tailscale_ssh && !machines[1].tailscale_ssh);
        assert_eq!(machines[2].os, "macOS");
        // Each is a box address as typed: a tailnet machine is named by its first name.
        let first = crate::BoxConfig::new("", &machines[0].endpoint, &[]).unwrap();
        assert_eq!((first.label.as_str(), first.endpoint.as_str()), ("forge", "ssh://forge.tail1234.ts.net"));
    }
}
