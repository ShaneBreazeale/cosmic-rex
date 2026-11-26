//! Why an `ssh://` box didn't connect, and the one-shot, explicitly chosen steps that fix it.
//!
//! Connecting never does any of this: each step runs only when the person picks it, once.
use rex_rune::ssh::{self, SetupOffer, Target, hostkey};

/// What stopped an `ssh://` box from connecting, as the core's last lookup found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// The box doesn't accept this computer's login: a key is needed.
    Auth,
    /// This computer hasn't trusted the box's host key yet.
    UnknownHostKey,
    /// The box's host key differs from the one trusted before. Never fixed from here.
    ChangedHostKey,
    /// Logged in, but no Rex server answers there.
    NoServer {
        /// Rex is installed there, so its personal server can be started.
        start: bool,
        /// A Linux login that isn't root, with no Rex: `rex server install` can set it up.
        install: bool,
    },
}

impl Problem {
    pub(crate) fn from_core(problem: &ssh::Problem, target: &Target) -> Self {
        match problem {
            ssh::Problem::AuthFailed => Self::Auth,
            ssh::Problem::UnknownKey => Self::UnknownHostKey,
            ssh::Problem::ChangedKey => Self::ChangedHostKey,
            ssh::Problem::NoServer(_) => {
                let offer = target.setup_offer(problem);
                Self::NoServer {
                    start: offer == Some(SetupOffer::Start),
                    install: offer == Some(SetupOffer::Install),
                }
            }
        }
    }

    /// A few words for the sidebar.
    pub fn badge(&self) -> &'static str {
        match self {
            Self::Auth => "! key",
            Self::UnknownHostKey => "! host key",
            Self::ChangedHostKey => "! host key changed",
            Self::NoServer { .. } => "! no Rex server",
        }
    }
}

/// A box's host key as scanned, for the person to compare and choose to trust.
#[derive(Clone, Debug)]
pub struct HostKey {
    offer: hostkey::Offer,
}

impl HostKey {
    /// The name OpenSSH records it under, such as `forge.tail1234.ts.net` or `[forge]:2222`.
    pub fn name(&self) -> &str {
        &self.offer.name
    }
    pub fn key_type(&self) -> &str {
        self.offer.key_type
    }
    /// `SHA256:…`, as `ssh` and `ssh-keygen -lf` print it.
    pub fn fingerprint(&self) -> &str {
        &self.offer.fingerprint
    }
    /// The `known_hosts` file it would be added to.
    pub fn file(&self) -> String {
        self.offer.file.display().to_string()
    }
}

/// Ask the box for its host key. Trusts nothing.
pub async fn scan_host_key(endpoint: String) -> Result<HostKey, String> {
    off_thread(async move {
        let target = target(&endpoint)?;
        hostkey::scan(&target, None).await.map(|offer| HostKey { offer }).map_err(words)
    })
    .await
}

/// Add the scanned key to `known_hosts`, on the person's word, after they compared it.
pub async fn trust_host_key(key: HostKey) -> Result<(), String> {
    off_thread(async move { hostkey::trust(&key.offer, None).await.map_err(words) }).await
}

/// What starting a personal server found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Started {
    /// It answers where the box's address finds it; connecting again reaches it.
    Ready,
    /// It answers, but not where the address looks; `endpoint` would reach it, if known.
    Elsewhere { reason: String, endpoint: Option<String> },
}

/// The command that starting runs, to show before the person chooses it.
pub fn start_preview(endpoint: &str) -> Result<String, String> {
    ssh::setup::command_preview(&target(endpoint)?).map_err(words)
}

/// Start the box's own Rex once, through this computer's `rex`. Nothing is left set to start
/// it again.
pub async fn start_server(endpoint: String) -> Result<Started, String> {
    off_thread(async move {
        match ssh::setup::start_personal(&endpoint, None).await {
            Ok(ssh::setup::Attachment::Automatic) => Ok(Started::Ready),
            Ok(ssh::setup::Attachment::Review { reason, endpoint }) => {
                Ok(Started::Elsewhere { reason: crate::plain(&reason, 400), endpoint })
            }
            Err(failure) => Err(crate::plain(&failure.to_string(), 400)),
        }
    })
    .await
}

/// Check that `rex server install` applies to the box, then give it as a program for a
/// terminal here, where Rex asks its own questions.
pub async fn install_program(endpoint: String) -> Result<Vec<String>, String> {
    off_thread(async move {
        let target = target(&endpoint)?;
        ssh::install::preflight(&target, None).await.map_err(|refusal| crate::plain(&refusal.to_string(), 400))?;
        let rex = rex_rune::platform::tools::program("rex").to_string_lossy().into_owned();
        let mut argv = vec![rex];
        argv.extend(ssh::install::install_args(&target).map_err(words)?);
        Ok(crate::keys::in_terminal(&argv.iter().map(String::as_str).collect::<Vec<_>>()))
    })
    .await
}

fn target(endpoint: &str) -> Result<Target, String> {
    match Target::parse(endpoint) {
        Ok(Some(target)) => Ok(target),
        Ok(None) => Err("Only an ssh:// box is set up this way".into()),
        Err(e) => Err(words(e)),
    }
}

fn words(error: impl std::fmt::Display) -> String {
    crate::plain(&format!("{error:#}"), 400)
}

/// The core's setup is tokio code; run it on a runtime of its own, so the UI's executor
/// doesn't matter.
async fn off_thread<T: Send + 'static>(
    work: impl std::future::Future<Output = Result<T, String>> + Send + 'static,
) -> Result<T, String> {
    let (done, wait) = flume::bounded(1);
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())
            .and_then(|runtime| runtime.block_on(work));
        let _ = done.send(result);
    });
    wait.recv_async().await.map_err(|_| "The step stopped".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_core_problem_has_its_own_badge_and_offer() {
        let target = Target::parse("ssh://me@forge").unwrap().unwrap();
        let facts = |os: &str, uid, rex| ssh::Facts { user: "me".into(), os: os.into(), uid: Some(uid), rex };
        let ours = |p: ssh::Problem| Problem::from_core(&p, &target);
        assert_eq!(ours(ssh::Problem::AuthFailed), Problem::Auth);
        assert_eq!(ours(ssh::Problem::UnknownKey).badge(), "! host key");
        assert_eq!(ours(ssh::Problem::ChangedKey), Problem::ChangedHostKey);
        assert_eq!(ours(ssh::Problem::NoServer(facts("Linux", 1000, true))), Problem::NoServer { start: true, install: false });
        assert_eq!(ours(ssh::Problem::NoServer(facts("Linux", 1000, false))), Problem::NoServer { start: false, install: true });
        assert_eq!(ours(ssh::Problem::NoServer(facts("Linux", 0, false))), Problem::NoServer { start: false, install: false });
        assert_eq!(ours(ssh::Problem::NoServer(facts("Darwin", 501, false))), Problem::NoServer { start: false, install: false });
        let named = Target::parse("ssh://me@forge/run/rex.sock").unwrap().unwrap();
        assert_eq!(
            Problem::from_core(&ssh::Problem::NoServer(facts("Linux", 1000, true)), &named),
            Problem::NoServer { start: false, install: false },
            "a named socket is its owner's to start"
        );
    }
}
