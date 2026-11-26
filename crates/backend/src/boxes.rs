//! The saved boxes (servers) and one connection to each.
use crate::{Host, Snapshot, Wake};
use serde::{Deserialize, Serialize};
use std::{
    io::Write as _,
    path::{Path, PathBuf},
};

/// How many identity tints there are; the frontend picks the colours.
pub const TINTS: u8 = 6;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxConfig {
    /// Stable and local; never an address.
    pub id: String,
    pub label: String,
    /// Two characters, unique, shown wherever typing can reach this box.
    pub monogram: String,
    #[serde(default)]
    pub tint: u8,
    /// `unix:///path`, `ssh://user@host` or `https://host`.
    pub endpoint: String,
    #[serde(default = "yes")]
    pub connect_on_start: bool,
}
fn yes() -> bool {
    true
}

impl BoxConfig {
    /// A new box named after its address, with a monogram and tint no other box has if possible.
    pub fn new(label: &str, endpoint: &str, others: &[BoxConfig]) -> Result<Self, String> {
        let endpoint = normalize(endpoint)?;
        let label = match label.trim() {
            "" => {
                let host = rex_rune::settings::endpoint_label(&endpoint);
                // A tailnet machine is known by its first name.
                match host.split_once('.') {
                    Some((name, rest)) if rest.ends_with(".ts.net") => name.to_owned(),
                    _ => host,
                }
            }
            given => given.to_owned(),
        };
        let label = crate::plain(&label, 32);
        let id = (1..)
            .map(|n| format!("box-{n}"))
            .find(|id| others.iter().all(|o| &o.id != id))
            .expect("unbounded");
        let tint = (0..TINTS).find(|t| others.iter().all(|o| o.tint != *t)).unwrap_or(0);
        let new = Self { id, monogram: monogram(&label, others), label, tint, endpoint, connect_on_start: true };
        check(&new, others)?;
        Ok(new)
    }

    /// This box with a new name or address, checked as a new one would be. It keeps its id,
    /// tint and start-up choice, and its monogram unless the name changed.
    pub fn edited(&self, label: &str, endpoint: &str, others: &[BoxConfig]) -> Result<Self, String> {
        let rest: Vec<BoxConfig> = others.iter().filter(|o| o.id != self.id).cloned().collect();
        let fresh = Self::new(label, endpoint, &rest)?;
        let monogram = if fresh.label == self.label { self.monogram.clone() } else { fresh.monogram };
        Ok(Self {
            id: self.id.clone(),
            monogram,
            tint: self.tint,
            connect_on_start: self.connect_on_start,
            ..fresh
        })
    }
}

/// Accept `user@host` as SSH; otherwise require a full address the core understands.
fn normalize(endpoint: &str) -> Result<String, String> {
    let given = endpoint.trim();
    if given.is_empty() {
        return Err("Enter an address, such as ssh://user@host".into());
    }
    let endpoint =
        if given.contains("://") { given.to_owned() } else { format!("ssh://{given}") };
    rex_rune::settings::validate_host("box", &endpoint).map_err(|e| crate::plain(&e.to_string(), 200))?;
    let lower = endpoint.to_ascii_lowercase();
    if let Some((scheme, rest)) = lower.split_once("://") {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if matches!(scheme, "http" | "ws" | "https" | "wss") && authority.contains('@') {
            return Err("Leave names and passwords out of web addresses".into());
        }
        if matches!(scheme, "http" | "ws") && !private(authority) {
            return Err("Plain http is only allowed to this computer or a Tailscale address; use https:// or ssh://".into());
        }
    }
    Ok(endpoint)
}

/// Loopback, or a tailnet (encrypted by Tailscale).
fn private(authority: &str) -> bool {
    use std::net::IpAddr;
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    };
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            ip.is_loopback() || (ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]))
        }
        Ok(IpAddr::V6(ip)) => ip.is_loopback(),
        Err(_) => host == "localhost" || host.ends_with(".ts.net"),
    }
}

fn monogram(label: &str, others: &[BoxConfig]) -> String {
    let letters: Vec<char> =
        label.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_uppercase).collect();
    let free = |m: &str| others.iter().all(|o| o.monogram != m);
    let first = letters.first().copied().unwrap_or('B');
    letters
        .iter()
        .skip(1)
        .copied()
        .chain('1'..='9')
        .chain('A'..='Z')
        .map(|second| format!("{first}{second}"))
        .find(|m| free(m))
        .unwrap_or_else(|| format!("{first}?"))
}

fn check(new: &BoxConfig, others: &[BoxConfig]) -> Result<(), String> {
    let clash = |o: &&BoxConfig| o.id != new.id && o.label.eq_ignore_ascii_case(&new.label);
    if new.label.trim().is_empty() {
        return Err("Enter a name".into());
    }
    if others.iter().any(|o| clash(&o)) {
        return Err(format!("A box named {} already exists", new.label));
    }
    if others.iter().any(|o| o.id != new.id && o.endpoint == new.endpoint) {
        return Err("That address is already in the list".into());
    }
    Ok(())
}

pub struct Boxes {
    path: Option<PathBuf>,
    boxes: Vec<(BoxConfig, Option<Host>)>,
}

impl Boxes {
    /// `$XDG_CONFIG_HOME/cosmic-rex/boxes.json`
    pub fn default_path() -> Option<PathBuf> {
        let absolute = |v: &str| std::env::var_os(v).map(PathBuf::from).filter(|p| p.is_absolute());
        absolute("XDG_CONFIG_HOME")
            .or_else(|| absolute("HOME").map(|h| h.join(".config")))
            .map(|dir| dir.join("cosmic-rex/boxes.json"))
    }

    /// A missing file is an empty list. A broken one is an error, so it is never overwritten.
    pub fn load(path: &Path) -> Result<Self, String> {
        let configs: Vec<BoxConfig> = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| format!("{} could not be read: {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(format!("{} could not be read: {e}", path.display())),
        };
        Ok(Self {
            path: Some(path.to_owned()),
            boxes: configs.into_iter().map(|c| (c, None)).collect(),
        })
    }

    /// Boxes for this run only; nothing is written.
    pub fn temporary(configs: Vec<BoxConfig>) -> Self {
        Self { path: None, boxes: configs.into_iter().map(|c| (c, None)).collect() }
    }

    pub fn configs(&self) -> Vec<BoxConfig> {
        self.boxes.iter().map(|(c, _)| c.clone()).collect()
    }
    pub fn iter(&self) -> impl Iterator<Item = &BoxConfig> {
        self.boxes.iter().map(|(c, _)| c)
    }
    pub fn config(&self, id: &str) -> Option<&BoxConfig> {
        self.boxes.iter().map(|(c, _)| c).find(|c| c.id == id)
    }
    pub fn host(&self, id: &str) -> Option<&Host> {
        self.boxes.iter().find(|(c, _)| c.id == id).and_then(|(_, h)| h.as_ref())
    }

    pub fn add(&mut self, new: BoxConfig) -> Result<(), String> {
        check(&new, &self.configs())?;
        self.boxes.push((new, None));
        self.save().inspect_err(|_| {
            self.boxes.pop();
        })
    }

    /// Save an edited box. A new address drops the old connection and, if there was one, opens
    /// one to the new address; nothing on either machine changes.
    pub fn replace(&mut self, edited: BoxConfig) -> Result<(), String> {
        let Some(at) = self.boxes.iter().position(|(c, _)| c.id == edited.id) else {
            return Err("That box is no longer in the list".into());
        };
        let others: Vec<BoxConfig> =
            self.configs().into_iter().filter(|c| c.id != edited.id).collect();
        check(&edited, &others)?;
        let old = std::mem::replace(&mut self.boxes[at].0, edited);
        if let Err(error) = self.save() {
            self.boxes[at].0 = old;
            return Err(error);
        }
        let (config, host) = &mut self.boxes[at];
        if config.endpoint != old.endpoint && host.take().is_some() {
            *host = Some(Host::connect(config.endpoint.clone(), None));
        }
        Ok(())
    }

    /// Forget a box here. Nothing on the box changes.
    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        let Some(at) = self.boxes.iter().position(|(c, _)| c.id == id) else { return Ok(()) };
        let removed = self.boxes.remove(at);
        if let Err(error) = self.save() {
            self.boxes.insert(at, removed);
            return Err(error);
        }
        Ok(())
    }

    /// Connect in the background, if not already.
    pub fn connect(&mut self, id: &str, session: Option<String>) {
        if let Some((config, host)) = self.boxes.iter_mut().find(|(c, _)| c.id == id)
            && host.is_none()
        {
            *host = Some(Host::connect(config.endpoint.clone(), session));
        }
    }
    pub fn connect_on_start(&mut self) {
        for (config, host) in &mut self.boxes {
            if config.connect_on_start && host.is_none() {
                *host = Some(Host::connect(config.endpoint.clone(), None));
            }
        }
    }
    /// Close cleanly, so the server drops this client at once.
    pub fn disconnect(&mut self, id: &str) {
        if let Some((_, host)) = self.boxes.iter_mut().find(|(c, _)| c.id == id) {
            *host = None;
        }
    }

    pub fn snapshots(&self) -> Vec<(String, Snapshot)> {
        self.boxes
            .iter()
            .filter_map(|(c, h)| Some((c.id.clone(), h.as_ref()?.snapshot())))
            .collect()
    }

    pub fn snapshot(&self, id: &str) -> Option<Snapshot> {
        self.host(id).map(Host::snapshot)
    }

    pub fn wakes(&self) -> Vec<(String, Wake)> {
        self.boxes
            .iter()
            .filter_map(|(c, h)| Some((c.id.clone(), h.as_ref()?)))
            .flat_map(|(id, h)| h.wakes().into_iter().map(move |w| (id.clone(), w)))
            .collect()
    }

    fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else { return Ok(()) };
        write(path, &self.configs()).map_err(|e| format!("{} could not be saved: {e}", path.display()))
    }
}

/// Replace the file in one step, readable only by this user.
fn write(path: &Path, configs: &[BoxConfig]) -> std::io::Result<()> {
    let dir = path.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
    std::fs::create_dir_all(dir)?;
    let temporary = path.with_extension(format!("json.{}", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let result = options.open(&temporary).and_then(|mut file| {
        file.write_all(&serde_json::to_vec_pretty(configs)?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_edit_keeps_identity_and_is_checked_like_a_new_box() {
        let a = BoxConfig::new("", "ssh://shane@100.122.107.79", &[]).unwrap();
        let b = BoxConfig::new("forge", "me@forge", std::slice::from_ref(&a)).unwrap();
        let all = [a.clone(), b.clone()];
        // Same name, new address: everything else stays.
        let moved = a.edited(&a.label, "https://mini-rex.tail1234.ts.net", &all).unwrap();
        assert_eq!(moved.endpoint, "https://mini-rex.tail1234.ts.net");
        assert_eq!((&moved.id, &moved.label, &moved.monogram, moved.tint), (&a.id, &a.label, &a.monogram, a.tint));
        // A blank name is taken from the new address, and the monogram follows it.
        let renamed = a.edited("", "https://mini-rex.tail1234.ts.net", &all).unwrap();
        assert_eq!(renamed.label, "mini-rex");
        assert_eq!(renamed.monogram, "MI");
        // Its own old values never clash with itself; another box's do.
        assert!(a.edited(&a.label, &a.endpoint, &all).is_ok());
        assert!(a.edited("FORGE", &a.endpoint, &all).is_err());
        assert!(a.edited(&a.label, "me@forge", &all).is_err());
        assert!(a.edited(&a.label, "http://example.com", &all).is_err());
    }

    #[test]
    fn replacing_a_box_saves_the_edit_and_refuses_a_clash() {
        let a = BoxConfig::new("mini", "ssh://shane@100.122.107.79", &[]).unwrap();
        let b = BoxConfig::new("forge", "me@forge", std::slice::from_ref(&a)).unwrap();
        let mut boxes = Boxes::temporary(vec![a.clone(), b.clone()]);
        let fixed = a.edited("mini", "https://mini-rex.tail1234.ts.net", &boxes.configs()).unwrap();
        boxes.replace(fixed.clone()).unwrap();
        assert_eq!(boxes.config(&a.id), Some(&fixed));
        // Built against a stale list, a clash is still caught when saved.
        let clash = BoxConfig { endpoint: b.endpoint.clone(), ..fixed.clone() };
        assert!(boxes.replace(clash).is_err());
        assert_eq!(boxes.config(&a.id), Some(&fixed));
        let gone = BoxConfig { id: "box-99".into(), ..fixed };
        assert!(boxes.replace(gone).is_err());
    }

    #[test]
    fn addresses_are_normalized_and_unsafe_ones_refused() {
        assert_eq!(normalize("me@forge").unwrap(), "ssh://me@forge");
        for ok in [
            "https://rex.tail1234.ts.net",
            "http://build.tail1234.ts.net:7297",
            "http://127.0.0.1:7297",
            "http://100.100.1.2:7297",
            "http://[::1]:7297",
            "unix:///run/rex/server.sock",
        ] {
            assert!(normalize(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "http://example.com",
            "ws://10.0.0.5:7297",
            "https://user:secret@example.com",
            "http://127.0.0.1@example.com",
            "http://localhost.example.com",
            "http://100.200.1.2",
            "ftp://example.com",
            "unix://relative",
        ] {
            assert!(normalize(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn identities_are_unique() {
        let mut all: Vec<BoxConfig> = Vec::new();
        for (label, at) in [("studio", "ssh://a"), ("stable", "ssh://b"), ("st", "ssh://c"), ("", "ssh://forge")] {
            let new = BoxConfig::new(label, at, &all).unwrap();
            assert!(all.iter().all(|o| o.monogram != new.monogram && o.tint != new.tint && o.id != new.id));
            all.push(new);
        }
        assert_eq!(all[0].monogram, "ST");
        assert_eq!(all[3].label, "forge");
        let tailnet = BoxConfig::new("", "https://mini-rex.tail1234.ts.net", &[]).unwrap();
        assert_eq!(tailnet.label, "mini-rex");
        assert!(BoxConfig::new("STUDIO", "ssh://z", &all).is_err(), "same name");
        assert!(BoxConfig::new("other", "ssh://a", &all).is_err(), "same address");
    }

    #[test]
    fn the_store_round_trips_privately_and_never_overwrites_a_broken_file() {
        let dir = std::env::temp_dir().join(format!("cosmic-rex-boxes-{}", std::process::id()));
        let path = dir.join("boxes.json");
        let mut boxes = Boxes::load(&path).unwrap();
        boxes.add(BoxConfig::new("studio", "ssh://studio", &[]).unwrap()).unwrap();
        assert_eq!(Boxes::load(&path).unwrap().configs(), boxes.configs());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        boxes.remove("box-1").unwrap();
        assert!(Boxes::load(&path).unwrap().configs().is_empty());
        std::fs::write(&path, "{oops").unwrap();
        assert!(Boxes::load(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{oops");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn one_unreachable_box_does_not_affect_another() {
        let make = |n: u8| BoxConfig {
            id: format!("box-{n}"),
            label: format!("b{n}"),
            monogram: format!("B{n}"),
            tint: n,
            endpoint: format!("unix:///nonexistent/{n}.sock"),
            connect_on_start: n == 1,
        };
        let mut boxes = Boxes::temporary(vec![make(1), make(2)]);
        boxes.connect_on_start();
        assert!(boxes.host("box-1").is_some() && boxes.host("box-2").is_none());
        boxes.connect("box-2", None);
        boxes.disconnect("box-1");
        assert_eq!(boxes.snapshots().len(), 1);
        assert_eq!(boxes.snapshots()[0].0, "box-2");
    }
}
