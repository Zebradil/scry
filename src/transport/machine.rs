use std::fmt;
use std::str::FromStr;

/// Where a command runs: this machine, or a target over `ssh`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Machine {
    /// The machine scry runs on. Commands run without `ssh` (spec 6.1, locality).
    Local,
    /// A target reached with the `ssh` binary.
    Remote(Remote),
}

impl Machine {
    /// The report's side `location`: `local`, or the target as in `root@web-a` (spec 8.1).
    pub fn location(&self) -> String {
        match self {
            Self::Local => "local".to_owned(),
            Self::Remote(remote) => remote.target.to_string(),
        }
    }
}

/// The form error messages use: `<path> is not in the store on this machine` (spec 9).
impl fmt::Display for Machine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local => f.write_str("this machine"),
            Self::Remote(remote) => remote.target.fmt(f),
        }
    }
}

/// A target plus the per-node connection options colmena may give. Two `Remote`s that differ
/// in any field are separate connections.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Remote {
    pub target: Target,
    /// colmena `deployment.targetPort`, passed as `-p <port>`.
    pub port: Option<u16>,
    /// colmena `deployment.sshOptions`, passed after `NIX_SSHOPTS` (spec 4.2, 6.1).
    pub ssh_options: Vec<String>,
}

/// `[user@]host` as given to `ssh`; a bare host may be an alias from `~/.ssh/config`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Target(String);

impl Target {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Target {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err("empty target".to_owned());
        }
        // ssh would read a leading `-` as an option, which a hive or a flag could abuse.
        if s.starts_with('-') {
            return Err(format!("target `{s}` starts with `-`"));
        }
        if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(format!("target `{s}` contains whitespace"));
        }
        // ssh splits at the last `@`.
        if let Some((user, host)) = s.rsplit_once('@')
            && (user.is_empty() || host.is_empty())
        {
            return Err(format!("target `{s}` is not [user@]host"));
        }
        Ok(Self(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_accepts_host_and_user_at_host() {
        for ok in ["web-a", "root@web-a", "web-a.example", "10.0.0.1", "::1"] {
            assert_eq!(ok.parse::<Target>().unwrap().as_str(), ok);
        }
    }

    #[test]
    fn target_rejects_option_like_and_malformed_values() {
        for bad in [
            "",
            "-oProxyCommand=x",
            "web a",
            "web-a\n",
            "@web-a",
            "root@",
        ] {
            assert!(bad.parse::<Target>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn location_and_message_forms() {
        let remote = Machine::Remote(Remote {
            target: "root@web-a".parse().unwrap(),
            port: Some(2222),
            ssh_options: vec![],
        });
        assert_eq!(remote.location(), "root@web-a");
        assert_eq!(remote.to_string(), "root@web-a");
        assert_eq!(Machine::Local.location(), "local");
        assert_eq!(Machine::Local.to_string(), "this machine");
    }
}
