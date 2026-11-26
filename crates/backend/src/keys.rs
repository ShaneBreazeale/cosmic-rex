//! This computer's SSH key, and making an `ssh://` box take it instead of a password.
//!
//! Steps that may ask something (a passphrase, the box's password, `sudo`, whether to trust a
//! host key) are programs for a terminal on this computer, so the person answers them there and
//! this app never sees a secret. Only the check runs here, and it never prompts.
use rex_rune::ssh::Target;
use std::path::{Path, PathBuf};

/// The keys `ssh` offers by default, best first.
const DEFAULT_KEYS: [&str; 3] = ["id_ed25519", "id_ecdsa", "id_rsa"];
/// Sorts before the drop-ins distributions ship (`50-cloud-init.conf` may allow passwords), and
/// sshd takes the first value it reads.
const KEYS_ONLY_FILE: &str = "/etc/ssh/sshd_config.d/00-cosmic-rex-keys-only.conf";

/// What logging in with only this computer's key found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyCheck {
    /// `sshd` let the key in with passwords refused.
    Works,
    /// Tailscale SSH answered in place of `sshd`: the tailnet's policy lets people in, and
    /// `sshd`'s password setting doesn't apply on the tailnet address.
    TailscaleSsh,
    /// It did not get in, with why.
    Fails(String),
}

/// This computer's public key, if it has one where `ssh` looks by default.
pub fn own_key() -> Option<PathBuf> {
    let dir = home()?.join(".ssh");
    DEFAULT_KEYS
        .iter()
        .map(|name| dir.join(name))
        .find(|private| private.is_file() && private.with_extension("pub").is_file())
        .map(|private| private.with_extension("pub"))
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).filter(|p| p.is_absolute())
}

/// Make an Ed25519 key where `ssh` finds it; `ssh-keygen` asks for a passphrase.
pub fn make_key_program() -> Result<Vec<String>, String> {
    let private = home().ok_or("HOME is not set")?.join(".ssh").join(DEFAULT_KEYS[0]);
    let private = private.to_string_lossy().into_owned();
    Ok(in_terminal(&["ssh-keygen", "-t", "ed25519", "-f", &private]))
}

/// Copy this computer's key to the box with `ssh-copy-id`, which logs in with the box's
/// password once (and asks to trust its host key the first time).
pub fn install_key_program(endpoint: &str) -> Result<Vec<String>, String> {
    let target = target(endpoint)?;
    let key = own_key().ok_or("Make a key first")?;
    let key = key.to_string_lossy();
    let mut argv = vec!["ssh-copy-id", "-i", &key];
    let port = target.port.map(|p| p.to_string());
    if let Some(port) = &port {
        argv.extend(["-p", port]);
    }
    let destination = target.destination();
    argv.extend(["--", &destination]);
    Ok(in_terminal(&argv))
}

/// Whether background logins can use this computer's key without asking for anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyState {
    /// No passphrase: `ssh` reads it as it is.
    Open,
    /// It has a passphrase, and the SSH agent holds it, so nothing is asked.
    InAgent,
    /// It has a passphrase and no agent holds it: connections here can't use it until
    /// it is added, since nothing can ask for the passphrase.
    Locked,
}

/// How this computer's key stands, or none without a key.
pub async fn key_state() -> Option<KeyState> {
    let (done, wait) = flume::bounded(1);
    std::thread::spawn(move || {
        let _ = done.send(key_state_now());
    });
    wait.recv_async().await.ok().flatten()
}

fn key_state_now() -> Option<KeyState> {
    let public = own_key()?;
    let quiet = |program: &str, args: &[&std::ffi::OsStr]| {
        std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
    };
    let private = public.with_extension("");
    // An empty passphrase only opens a key that has none.
    let open = quiet("ssh-keygen", &["-y".as_ref(), "-P".as_ref(), "".as_ref(), "-f".as_ref(), private.as_os_str()])
        .is_some_and(|o| o.status.success());
    if open {
        return Some(KeyState::Open);
    }
    let body = std::fs::read_to_string(&public).ok()?;
    let body = body.split_whitespace().nth(1)?.to_owned();
    let held = quiet("ssh-add", &["-L".as_ref()])
        .is_some_and(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.split_whitespace().nth(1) == Some(&body)));
    Some(if held { KeyState::InAgent } else { KeyState::Locked })
}

/// Add this computer's key to the SSH agent; `ssh-add` asks for its passphrase.
pub fn add_to_agent_program() -> Result<Vec<String>, String> {
    let private = own_key().ok_or("Make a key first")?.with_extension("");
    Ok(in_terminal(&["ssh-add", &private.to_string_lossy()]))
}

/// Log in with only this computer's key, never prompting, and see what answered.
pub async fn check_key(endpoint: String) -> KeyCheck {
    let (done, wait) = flume::bounded(1);
    std::thread::spawn(move || {
        let _ = done.send(check_key_now(&endpoint));
    });
    wait.recv_async().await.unwrap_or_else(|_| KeyCheck::Fails("The check stopped".into()))
}

fn check_key_now(endpoint: &str) -> KeyCheck {
    let (target, key) = match (target(endpoint), own_key()) {
        (Err(e), _) => return KeyCheck::Fails(e),
        (_, None) => return KeyCheck::Fails("This computer has no key yet".into()),
        (Ok(t), Some(k)) => (t, k),
    };
    let output = std::process::Command::new("ssh")
        .args(check_args(&target, &key.with_extension("")))
        .stdin(std::process::Stdio::null())
        .output();
    match output {
        Ok(out) => judge(out.status.success(), &String::from_utf8_lossy(&out.stderr)),
        Err(e) => KeyCheck::Fails(format!("Couldn't run ssh: {e}")),
    }
}

/// Verbose, so the server's software version shows; batch, so nothing is asked; public keys
/// only, so a password can't stand in; strict, so an untrusted host key fails here too.
fn check_args(target: &Target, private: &Path) -> Vec<String> {
    let mut args: Vec<String> = [
        "-v",
        "-o", "BatchMode=yes",
        "-o", "StrictHostKeyChecking=yes",
        "-o", "PreferredAuthentications=publickey",
        "-o", "PasswordAuthentication=no",
        "-o", "KbdInteractiveAuthentication=no",
        "-o", "IdentitiesOnly=yes",
        "-o", "ConnectTimeout=10",
        "-o", "ControlPath=none",
        "-i",
    ]
    .map(str::to_owned)
    .into();
    args.push(private.to_string_lossy().into_owned());
    if let Some(port) = target.port {
        args.extend(["-p".to_owned(), port.to_string()]);
    }
    args.extend(["--".to_owned(), target.destination(), "true".to_owned()]);
    args
}

fn judge(success: bool, stderr: &str) -> KeyCheck {
    if stderr.lines().any(|l| l.contains("remote software version Tailscale")) {
        return KeyCheck::TailscaleSsh;
    }
    if success {
        return KeyCheck::Works;
    }
    let said = stderr
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty() && !l.starts_with("debug") && !l.starts_with("OpenSSH_"))
        .unwrap_or("ssh could not log in");
    let why = if said.contains("Permission denied") {
        "The key was not accepted. Install it first, or add it to your SSH agent if it has a passphrase."
    } else if said.contains("Host key verification failed") || said.contains("host key is known") {
        "This computer doesn't trust the box's host key yet. Installing the key asks you to check it."
    } else {
        said
    };
    KeyCheck::Fails(crate::plain(why, 300))
}

/// Turn off password and keyboard-interactive logins on the box's `sshd`, through `sudo`.
/// `sshd` checks the change before it is reloaded, and it is taken back out if `sshd`
/// rejects it or another setting would still allow passwords. Only offer this once
/// [`check_key`] says [`KeyCheck::Works`].
pub fn keys_only_program(endpoint: &str) -> Result<Vec<String>, String> {
    let target = target(endpoint)?;
    let mut argv = vec!["ssh".to_owned(), "-t".to_owned()];
    if let Some(port) = target.port {
        argv.extend(["-p".to_owned(), port.to_string()]);
    }
    argv.extend(["--".to_owned(), target.destination(), format!("sudo sh -c '{}'", keys_only_script())]);
    Ok(in_terminal(&argv.iter().map(String::as_str).collect::<Vec<_>>()))
}

/// Run on the box as root. No single quotes, so it can sit inside them.
fn keys_only_script() -> String {
    let script = r##"set -e
f=FILE
if ! grep -Eiq "^[[:space:]]*Include[[:space:]]+/etc/ssh/sshd_config\.d/" /etc/ssh/sshd_config; then
  echo "/etc/ssh/sshd_config does not read /etc/ssh/sshd_config.d, so nothing was changed."; exit 1
fi
printf "# Written by cosmic-rex: log in with keys only.\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n" > "$f.new"
chmod 644 "$f.new"
mv "$f.new" "$f"
if ! sshd -t; then rm -f "$f"; echo "sshd rejected the change, so it was taken back out."; exit 1; fi
if ! sshd -T 2>/dev/null | grep -qx "passwordauthentication no"; then
  rm -f "$f"; echo "Another sshd setting still allows passwords, so nothing was changed."; exit 1
fi
if systemctl reload ssh 2>/dev/null || systemctl reload sshd 2>/dev/null; then
  echo "Password login is off. Your key still works; this session stays open."
else
  echo "Saved. No systemd here to reload sshd: on macOS the next login uses it; elsewhere, restart sshd."
fi"##;
    script.replace("FILE", KEYS_ONLY_FILE)
}

fn target(endpoint: &str) -> Result<Target, String> {
    match Target::parse(endpoint) {
        Ok(Some(target)) => Ok(target),
        Ok(None) => Err("Only an ssh:// box takes a key".into()),
        Err(e) => Err(crate::plain(&e.to_string(), 200)),
    }
}

/// Run `argv` and keep its last words on screen until Enter, since its pane closes with it.
pub(crate) fn in_terminal(argv: &[&str]) -> Vec<String> {
    let wrapper = r#""$@"; s=$?; echo; if [ "$s" -eq 0 ]; then echo "Done."; else echo "It ended with status $s."; fi; printf "Press Enter to close. "; read -r _"#;
    ["sh", "-c", wrapper, "sh"].iter().chain(argv).map(|s| (*s).to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_take_their_arguments_unquoted_and_nothing_secret() {
        let argv = in_terminal(&["ssh-copy-id", "-i", "/home/me/.ssh/id_ed25519.pub", "--", "me@forge"]);
        assert_eq!(&argv[..2], ["sh", "-c"]);
        assert_eq!(&argv[3..], ["sh", "ssh-copy-id", "-i", "/home/me/.ssh/id_ed25519.pub", "--", "me@forge"]);
        let off = keys_only_program("ssh://me@forge.tail1234.ts.net:2222").unwrap();
        let at = off.iter().position(|a| a == "ssh").unwrap();
        assert_eq!(&off[at..at + 6], ["ssh", "-t", "-p", "2222", "--", "me@forge.tail1234.ts.net"]);
        assert!(off.last().unwrap().starts_with("sudo sh -c '") && off.last().unwrap().ends_with('\''));
        assert!(!keys_only_script().contains('\''), "it sits in single quotes");
        assert!(keys_only_program("https://forge.tail1234.ts.net").is_err());
    }

    #[test]
    fn the_check_never_prompts_and_offers_only_the_key() {
        let target = target("ssh://forge:2222").unwrap();
        let args = check_args(&target, Path::new("/k/id_ed25519"));
        for must in ["BatchMode=yes", "StrictHostKeyChecking=yes", "PasswordAuthentication=no", "IdentitiesOnly=yes"] {
            assert!(args.iter().any(|a| a == must), "{must}");
        }
        assert_eq!(&args[args.len() - 6..], ["/k/id_ed25519", "-p", "2222", "--", "forge", "true"]);
    }

    #[test]
    fn what_answered_is_told_apart() {
        let tailscale = "debug1: Remote protocol version 2.0, remote software version Tailscale\n";
        assert_eq!(judge(true, tailscale), KeyCheck::TailscaleSsh);
        let sshd = "debug1: Remote protocol version 2.0, remote software version OpenSSH_9.6p1\n";
        assert_eq!(judge(true, sshd), KeyCheck::Works);
        let denied = format!("{sshd}debug1: No more authentication methods to try.\nme@forge: Permission denied (publickey,password).\n");
        assert!(matches!(judge(false, &denied), KeyCheck::Fails(why) if why.starts_with("The key was not accepted")));
        assert!(matches!(judge(false, "Host key verification failed.\n"), KeyCheck::Fails(why) if why.contains("host key")));
    }
}
