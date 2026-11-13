//! A synthetic fleet on the fake seam (spec 11): hand-written closures per machine, and a
//! [`Fake`] that answers [`fetch::SCRIPT`] in every mode by computing what the real script prints
//! for them: `find`/`ls -ldn`/`cksum` lines, `nix path-info --json` with references scanned from
//! contents and link targets as Nix does, and real `tar` streams, gzipped when the host has
//! `gzip`. Pipeline tests and the fixture run mode (M8) build on it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::sync::{Arc, Mutex};

use crate::exec::Output;
use crate::exec::fake::Fake;
use crate::fetch::{self, probe::CURRENT_SYSTEM};
use crate::files::churn::store_refs;
use crate::store::name::STORE_PREFIX;
use crate::transport::machine::Machine;
use crate::tree::LISTING;

/// A store path with a hash derived from `seed` and `name`, from Nix's base32 alphabet.
pub fn store_path(seed: &str, name: &str) -> String {
    const BASE32: &[u8] = b"0123456789abcdfghijklmnpqrsvwxyz";
    let mut state = format!("{seed}/{name}")
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        });
    let hash: String = (0..32)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            char::from(BASE32[(state % 32) as usize])
        })
        .collect();
    format!("{STORE_PREFIX}{hash}-{name}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    File(Vec<u8>),
    Dir,
    Link(String),
    /// A directory `find` cannot read.
    Unreadable,
}

/// One machine's filesystem and environment.
#[derive(Debug, Clone)]
pub struct Host {
    nodes: BTreeMap<String, Node>,
    nar_sizes: BTreeMap<String, u64>,
    pub home: String,
    pub user: String,
    pub xdg_state_home: Option<String>,
    pub gzip: bool,
    pub nix: bool,
    /// `ssh`'s error line when the machine cannot be reached.
    pub unreachable: Option<String>,
}

impl Default for Host {
    fn default() -> Self {
        Self {
            nodes: BTreeMap::from([("/".to_owned(), Node::Dir)]),
            nar_sizes: BTreeMap::new(),
            home: "/root".to_owned(),
            user: "root".to_owned(),
            xdg_state_home: None,
            gzip: true,
            nix: true,
            unreachable: None,
        }
    }
}

impl Host {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a regular file, and the directories above it.
    pub fn file(&mut self, path: &str, content: impl Into<Vec<u8>>) -> &mut Self {
        self.put(path, Node::File(content.into()))
    }

    pub fn link(&mut self, path: &str, target: &str) -> &mut Self {
        self.put(path, Node::Link(target.to_owned()))
    }

    pub fn dir(&mut self, path: &str) -> &mut Self {
        self.put(path, Node::Dir)
    }

    pub fn unreadable(&mut self, path: &str) -> &mut Self {
        self.put(path, Node::Unreadable)
    }

    /// Overrides the computed `narSize` of a store path.
    pub fn nar_size(&mut self, store_path: &str, size: u64) -> &mut Self {
        self.nar_sizes.insert(store_path.to_owned(), size);
        self
    }

    /// Panics when a parent is not a directory: that path would not be reachable.
    fn put(&mut self, path: &str, node: Node) -> &mut Self {
        let mut parent = path;
        while let Some(i) = parent.rfind('/') {
            parent = if i == 0 { "/" } else { &parent[..i] };
            match self.nodes.entry(parent.to_owned()).or_insert(Node::Dir) {
                Node::Dir => {}
                other => panic!("{parent} is {other:?}, so {path} cannot be under it"),
            }
            if parent == "/" {
                break;
            }
        }
        self.nodes.insert(path.to_owned(), node);
        self
    }

    /// `lstat`: the parent resolved through links, the last component not. The canonical path
    /// and its node.
    fn lstat(&self, path: &str) -> Option<(String, &Node)> {
        // Lexical, like the walk's own link resolution.
        let path = normalize(path);
        if path == "/" {
            return self.nodes.get("/").map(|n| (path.clone(), n));
        }
        let (parent, name) = path.rsplit_once('/')?;
        let parent = if parent.is_empty() {
            String::new()
        } else {
            self.resolve(parent)?
        };
        let canonical = format!("{}/{name}", parent.trim_end_matches('/'));
        self.nodes.get(&canonical).map(|n| (canonical, n))
    }

    /// `realpath`: every link followed; `None` when anything on the way is missing.
    fn resolve(&self, path: &str) -> Option<String> {
        let mut path = path.to_owned();
        for _ in 0..40 {
            let (canonical, node) = self.lstat(&path)?;
            match node {
                Node::Link(target) if target.starts_with('/') => path.clone_from(target),
                Node::Link(target) => {
                    let dir = &canonical[..canonical.rfind('/')?];
                    path = format!("{dir}/{target}");
                }
                _ => return Some(canonical),
            }
        }
        None
    }

    /// `stat`, following links: what `[ -e ]`, `-f` and `-d` look at.
    fn stat(&self, path: &str) -> Option<&Node> {
        self.nodes.get(&self.resolve(path)?)
    }

    /// [`LISTING`] on `roots`: stdout, stderr and the exit status.
    fn list(&self, roots: &[String]) -> (String, String, i32) {
        let (mut out, mut err, mut status) = (String::new(), String::new(), 0);
        for root in roots {
            let Some((canonical, node)) = self.lstat(root) else {
                continue;
            };
            let mut entries = vec![(root.clone(), node)];
            if matches!(node, Node::Dir) {
                let prefix = format!("{canonical}/");
                for (path, node) in self.nodes.range(prefix.clone()..) {
                    let Some(rest) = path.strip_prefix(&prefix) else {
                        break;
                    };
                    let below_unreadable = entries.iter().any(|(p, n)| {
                        matches!(n, Node::Unreadable)
                            && format!("{root}/{rest}").starts_with(&format!("{p}/"))
                    });
                    if !below_unreadable {
                        entries.push((format!("{root}/{rest}"), node));
                    }
                }
            }
            for (path, node) in entries {
                if path.contains('\n') {
                    out.push_str(&format!("newline: {path}\n"));
                    continue;
                }
                match node {
                    Node::Dir => out.push_str(&format!("{path}\n")),
                    Node::Unreadable => {
                        out.push_str(&format!("{path}\n"));
                        err.push_str(&format!("find: {path}: Permission denied\n"));
                        status = 1;
                    }
                    Node::File(data) => {
                        out.push_str(&format!("{} {} {path}\n", cksum(data), data.len()))
                    }
                    Node::Link(target) => out.push_str(&format!(
                        "lrwxrwxrwx 1 0 0 {} Jan  1  1970 {path} -> {target}\n",
                        target.len()
                    )),
                }
            }
        }
        (out, err, status)
    }

    /// The nodes at and below the store path `root`, by canonical path.
    fn store_nodes<'a>(&'a self, root: &'a str) -> impl Iterator<Item = (&'a String, &'a Node)> {
        let prefix = format!("{root}/");
        self.nodes
            .range(root.to_owned()..)
            .take_while(move |(p, _)| *p == root || p.starts_with(&prefix))
    }

    /// `nix path-info --json -r` of the store path `toplevel`; references are the store paths
    /// named in contents and link targets, as Nix scans them.
    fn path_info(&self, toplevel: &str) -> String {
        let mut closure = BTreeSet::new();
        let mut queue = vec![toplevel.to_owned()];
        while let Some(path) = queue.pop() {
            if !self.nodes.contains_key(&path) || !closure.insert(path.clone()) {
                continue;
            }
            for (_, node) in self.store_nodes(&path) {
                let text = match node {
                    Node::File(data) => String::from_utf8_lossy(data).into_owned(),
                    Node::Link(target) => target.clone(),
                    Node::Dir | Node::Unreadable => continue,
                };
                queue.extend(
                    store_refs(&text)
                        .into_iter()
                        .map(|r| text[r.range].to_owned()),
                );
            }
        }
        if closure.is_empty() {
            return serde_json::json!({ toplevel: null }).to_string();
        }
        let entries: serde_json::Map<String, serde_json::Value> = closure
            .into_iter()
            .map(|path| {
                let size = self.nar_sizes.get(&path).copied().unwrap_or_else(|| {
                    self.store_nodes(&path)
                        .map(|(_, node)| match node {
                            Node::File(data) => data.len() as u64 + 120,
                            Node::Link(target) => target.len() as u64 + 100,
                            Node::Dir | Node::Unreadable => 100,
                        })
                        .sum()
                });
                (path, serde_json::json!({ "narSize": size }))
            })
            .collect();
        serde_json::Value::Object(entries).to_string()
    }

    /// `tar -c -h -f - -T -` on `paths`, through `gzip -1` when asked: stdout and stderr.
    fn tar(&self, paths: &[String], gzip: bool) -> (Vec<u8>, String) {
        let mut err = String::new();
        let mut builder = tar::Builder::new(Vec::new());
        let mut status = 0;
        for path in paths {
            if path.starts_with('/') && err.is_empty() {
                err.push_str("tar: Removing leading `/' from member names\n");
            }
            let Some(Node::File(data)) = self.stat(path) else {
                err.push_str(&format!(
                    "tar: {path}: Cannot stat: No such file or directory\n"
                ));
                status = 2;
                continue;
            };
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o444);
            header.set_cksum();
            builder
                .append_data(&mut header, path.trim_start_matches('/'), &data[..])
                .expect("in memory");
        }
        let mut out = builder.into_inner().expect("in memory");
        if status != 0 {
            err.push_str(&format!("::scry failed tar {status}\n"));
        }
        if gzip {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            encoder.write_all(&out).expect("in memory");
            out = encoder.finish().expect("in memory");
        }
        (out, err)
    }

    /// The `probe` mode, line for line as the script.
    fn probe(&self, stdin: &str) -> String {
        let mut out = String::new();
        let mut seen = BTreeSet::new();
        for line in stdin.lines() {
            out.push_str("::scry side\n");
            let path = if line.is_empty() {
                let state = self
                    .xdg_state_home
                    .clone()
                    .unwrap_or_else(|| format!("{}/.local/state", self.home));
                let xdg = format!("{state}/nix/profiles/home-manager");
                let per_user = format!("/nix/var/nix/profiles/per-user/{}/home-manager", self.user);
                if self.stat(&xdg).is_none() && self.stat(&per_user).is_some() {
                    per_user
                } else {
                    xdg
                }
            } else {
                line.to_owned()
            };
            out.push_str(&format!("::scry path {path}\n"));
            let in_store = |t: &String| {
                t.strip_prefix(STORE_PREFIX)
                    .is_some_and(|name| !name.is_empty() && !name.contains('/'))
            };
            let Some(toplevel) = self
                .resolve(&path)
                .filter(|t| matches!(self.nodes.get(t), Some(Node::Dir | Node::Unreadable)))
                .filter(in_store)
            else {
                continue;
            };
            out.push_str(&format!("::scry toplevel {toplevel}\n"));
            if !seen.insert(toplevel.clone()) {
                continue;
            }
            out.push_str("::scry listing\n");
            let (listing, err, status) = self.list(std::slice::from_ref(&toplevel));
            out.push_str(&listing);
            if status != 0 || !err.is_empty() {
                out.push_str(&format!("::scry failed find {status}\n{err}\n"));
                return out;
            }
            for file in [
                "nixos-version",
                "darwin-version",
                "hm-version",
                "kernel-params",
            ] {
                if let Some(Node::File(data)) = self.stat(&format!("{toplevel}/{file}")) {
                    out.push_str(&format!("::scry file {file}\n"));
                    out.push_str(&String::from_utf8_lossy(data));
                    out.push('\n');
                }
            }
            out.push_str("::scry pathinfo\n");
            if !self.nix {
                out.push_str("::scry failed nix 127\nsh: 1: nix: not found\n\n");
                return out;
            }
            out.push_str(&self.path_info(&toplevel));
            out.push_str("\n\n");
        }
        if self.gzip {
            out.push_str("::scry gzip\n");
        }
        out
    }

    /// The `refs` mode: listing, then the archive of regular files below `limit` bytes.
    fn refs(&self, stdin: &str, gzip: bool, limit: u64) -> Output {
        let mut out = Vec::new();
        let mut roots = Vec::new();
        for line in stdin.lines() {
            let (take, root) = line.split_at(2);
            if matches!(self.stat(root), Some(Node::Dir | Node::Unreadable)) {
                if take.starts_with('f') {
                    continue;
                }
                out.extend(format!("{root}\n").bytes());
                roots.push(format!("{root}/bin"));
            } else if !take.starts_with('b') {
                roots.push(root.to_owned());
            }
        }
        let (listing, err, status) = self.list(&roots);
        out.extend(listing.bytes());
        if status != 0 || !err.is_empty() {
            out.extend(format!("::scry failed find {status}\n{err}\n").bytes());
            return output(1, out, "");
        }
        out.extend(b"::scry tar\n");
        // `find -type f -size -<limit>c ! -name "*[[-^]*"`, roots not followed.
        let wanted = |path: &str, data: &Vec<u8>| {
            let name = &path[path.rfind('/').map_or(0, |i| i + 1)..];
            (data.len() as u64) < limit && !name.contains(['[', '\\', ']', '^'])
        };
        let mut files = Vec::new();
        for root in &roots {
            let Some((canonical, node)) = self.lstat(root) else {
                continue;
            };
            match node {
                Node::File(data) if wanted(root, data) => files.push(root.clone()),
                Node::Dir => {
                    for (path, node) in self.store_nodes(&canonical) {
                        if let Node::File(data) = node
                            && wanted(path, data)
                        {
                            files.push(format!("{root}{}", &path[canonical.len()..]));
                        }
                    }
                }
                _ => {}
            }
        }
        let (archive, err) = self.tar(&files, gzip);
        out.extend(archive);
        output(0, out, &err)
    }

    fn answer(&self, args: &[String], stdin: &[u8]) -> Output {
        let arg = |i: usize| args.get(i).map_or("", String::as_str);
        let stdin = String::from_utf8_lossy(stdin);
        let paths: Vec<String> = stdin.lines().map(str::to_owned).collect();
        match arg(3) {
            "probe" => output(0, self.probe(&stdin), ""),
            "list" => {
                let (mut out, err, status) = self.list(&paths);
                if status != 0 || !err.is_empty() {
                    out.push_str(&format!("::scry failed find {status}\n{err}\n"));
                }
                output(status, out, "")
            }
            "refs" => self.refs(&stdin, arg(5) == "gzip", arg(6).parse().unwrap_or(u64::MAX)),
            "content" => {
                let (out, err) = self.tar(&paths, arg(5) == "gzip");
                output(0, out, &err)
            }
            mode => panic!("unknown fetch mode {mode:?}"),
        }
    }
}

fn output(code: i32, stdout: impl Into<Vec<u8>>, stderr: &str) -> Output {
    Output {
        code: Some(code),
        stdout: stdout.into(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    format!("/{}", parts.join("/"))
}

/// POSIX `cksum`: CRC-32 with polynomial 0x04C11DB7 over the data and then its length.
pub fn cksum(data: &[u8]) -> u32 {
    let mut crc = 0u32;
    let mut feed = |byte: u8| {
        crc ^= u32::from(byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    };
    data.iter().for_each(|&b| feed(b));
    let mut len = data.len();
    while len > 0 {
        feed(len as u8);
        len >>= 8;
    }
    !crc
}

/// Machines and their hosts, with the round trips each one answered.
#[derive(Default)]
pub struct Fleet {
    hosts: HashMap<Machine, Host>,
    round_trips: Mutex<HashMap<Machine, usize>>,
}

impl Fleet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn host(mut self, machine: Machine, host: Host) -> Self {
        self.hosts.insert(machine, host);
        self
    }

    /// Commands [`fetch::SCRIPT`] ran on `machine` so far.
    pub fn round_trips(&self, machine: &Machine) -> usize {
        self.round_trips
            .lock()
            .expect("not poisoned")
            .get(machine)
            .copied()
            .unwrap_or(0)
    }

    /// A fake seam answering [`fetch::SCRIPT`] on the fleet's machines. Other commands find
    /// no rule here; add rules for them to the returned fake.
    pub fn fake(self: &Arc<Self>) -> Fake {
        let fleet = Arc::clone(self);
        Fake::new().rule(move |machine, command| {
            let is_fetch = command.program == "sh"
                && command.args.get(1).map(String::as_str) == Some(fetch::SCRIPT.text());
            let host = fleet.hosts.get(machine).filter(|_| is_fetch)?;
            if let Some(listing) = command.args.get(4).filter(|l| !l.is_empty()) {
                assert_eq!(
                    listing,
                    LISTING.text(),
                    "the listing script is passed as is"
                );
            }
            *fleet
                .round_trips
                .lock()
                .expect("not poisoned")
                .entry(machine.clone())
                .or_default() += 1;
            if let Some(line) = &host.unreachable {
                return Some(output(255, "", &format!("ssh: {line}\n")));
            }
            Some(host.answer(&command.args, &command.stdin))
        })
    }
}

/// One NixOS generation of `web-a`: `old` before a nixpkgs bump and a config change, `!old`
/// after. Link depth 3 (`etc` → `etc/systemd/system` → `app.service`); followed references to
/// depth 2 (`app.service` → `app-start/bin/app-start` → `app.json`), but neither `app-start`'s
/// reference to itself, nor `app-helper`'s (a small rebuild), nor `activate`'s to the toplevel.
pub fn nixos(h: &mut Host, old: bool) -> String {
    let s = if old { "o" } else { "n" };
    let pick = |o: &'static str, n: &'static str| if old { o } else { n };
    let bash = store_path(s, "bash-5.2p37");
    h.file(&format!("{bash}/bin/bash"), b"\x7fELF\0bash".to_vec())
        .nar_size(&bash, 2 << 20);
    let openssl = store_path(s, pick("openssl-3.0.13", "openssl-3.0.14"));
    h.file(
        &format!("{openssl}/lib/libssl.so"),
        b"\x7fELF\0ssl".to_vec(),
    );
    let app_json = store_path(s, "app.json");
    h.file(&app_json, pick("{\"workers\": 4}\n", "{\"workers\": 8}\n"));
    let app_start = store_path(s, "app-start");
    h.file(
        &format!("{app_start}/bin/app-start"),
        format!(
            "#!{bash}/bin/bash\nset -eu\ncd {app_start}\nexec app --config {app_json} --workers {}\n",
            pick("4", "8")
        ),
    );
    h.file(&format!("{app_start}/bin/app"), pick("\0old", "\0new"));
    let helper = store_path(s, "app-helper");
    h.file(
        &format!("{helper}/bin/app-helper"),
        format!(
            "#!{bash}/bin/bash\n# from {helper}\necho {}\n",
            pick("v1", "v2")
        ),
    );
    let unit = store_path(s, "unit-app.service");
    h.file(
        &format!("{unit}/app.service"),
        format!("[Service]\nExecStart={app_start}/bin/app-start\n"),
    );
    let units = store_path(s, "system-units");
    h.link(
        &format!("{units}/app.service"),
        &format!("{unit}/app.service"),
    );
    let debug_shell = format!("{units}/debug-shell.service");
    if old {
        let debug_unit = store_path(s, "unit-debug-shell.service");
        h.file(&format!("{debug_unit}/debug-shell.service"), "[Unit]\n");
        h.link(&debug_shell, &format!("{debug_unit}/debug-shell.service"));
    } else {
        h.link(&debug_shell, "/dev/null");
    }
    let hosts = store_path("shared", "hosts");
    h.file(&hosts, "127.0.0.1 localhost\n");
    let nix_conf = store_path(s, "nix.conf");
    h.file(
        &nix_conf,
        format!(
            "substituters = https://cache.example\ntrusted-public-keys = cache-a:AAAA{}\nmax-jobs = auto\n",
            pick("", " cache-b:BBBB")
        ),
    );
    let profile = store_path(s, "profile");
    h.file(&profile, format!("export PATH={bash}/bin:$PATH\n"));
    let openssl_cnf = store_path(s, "openssl.cnf");
    h.file(
        &openssl_cnf,
        format!("include {openssl}/etc/ssl/openssl.cnf\n"),
    );
    let user_env = store_path(s, "user-environment");
    h.file(&format!("{user_env}/bin/tool"), pick("a", "b"));

    let etc = store_path(s, "etc");
    let e = |rel: &str| format!("{etc}/etc/{rel}");
    h.link(&e("hosts"), &hosts)
        .link(&e("nix/nix.conf"), &nix_conf)
        .link(&e("profile"), &profile)
        .link(&e("ssl/openssl.cnf"), &openssl_cnf)
        .link(&e("systemd/system"), &units)
        .link(&e("mtab"), pick("/proc/mounts", "/proc/self/mounts"))
        .link(&e("fonts/fonts.conf"), "/etc/fonts/fonts.conf")
        .link(&e("profiles/per-user/root"), &user_env)
        .file(
            &e("udev/hwdb.bin"),
            vec![if old { b'a' } else { b'b' }; 5000],
        )
        .file(&e("binary.db"), pick("\0\x01", "\0\x02"))
        .file(
            &e("systemd/system-generators/dev-disk-by\\x2dlabel.mount"),
            pick("[Mount]\n", "[Mount]\nOptions=ro\n"),
        );
    if old {
        h.link(
            &e("resolv.conf"),
            &format!("{}/resolv.conf", store_path("o", "gone")),
        )
        .link(&e("gone-everywhere"), &store_path("o", "never"));
    } else {
        let resolv = store_path("n", "resolv.conf");
        h.file(&resolv, "nameserver 10.0.0.1\n")
            .link(&e("resolv.conf"), &resolv)
            .link(&e("gone-everywhere"), &store_path("n", "never"))
            .file(
                &e("ssh/authorized_keys.d/deploy"),
                "ssh-ed25519 AAAA deploy@example\n",
            );
    }

    let kernel = pick("6.12.40", "6.12.48");
    let linux = store_path(s, &format!("linux-{kernel}"));
    let initrd = store_path(s, &format!("initrd-linux-{kernel}"));
    let modules = store_path(s, "linux-6.12.48-modules");
    for (path, file) in [(&linux, "bzImage"), (&initrd, "initrd"), (&modules, "lib")] {
        h.file(&format!("{path}/{file}"), b"\0kernel".to_vec())
            .nar_size(path, 12 << 20);
    }
    let sw = store_path(s, "system-path");
    h.link(&format!("{sw}/bin/bash"), &format!("{bash}/bin/bash"))
        .link(
            &format!("{sw}/bin/app-helper"),
            &format!("{helper}/bin/app-helper"),
        )
        .link(
            &format!("{sw}/lib/libssl.so"),
            &format!("{openssl}/lib/libssl.so"),
        );
    if !old {
        let agent = store_path("n", "new-agent-1.4.0");
        h.file(&format!("{agent}/bin/new-agent"), "\0agent").link(
            &format!("{sw}/bin/new-agent"),
            &format!("{agent}/bin/new-agent"),
        );
    }

    let version = pick("26.05.20261102.7dcdd87", "26.11.20261110.9a8b7c6");
    let t = store_path(s, &format!("nixos-system-web-a-{version}"));
    h.file(&format!("{t}/nixos-version"), format!("{version}\n"))
        .file(
            &format!("{t}/kernel-params"),
            pick("quiet loglevel=4", "quiet  loglevel=4 mitigations=auto\n"),
        )
        .link(&format!("{t}/kernel"), &format!("{linux}/bzImage"))
        .link(&format!("{t}/initrd"), &format!("{initrd}/initrd"))
        .link(&format!("{t}/kernel-modules"), &format!("{modules}/lib"))
        .link(&format!("{t}/etc"), &format!("{etc}/etc"))
        .link(&format!("{t}/sw"), &sw)
        .file(
            &format!("{t}/activate"),
            format!("#!{bash}/bin/bash\nsystemConfig={t}\n"),
        );
    if !old {
        let debug = store_path("n", "nixos-system-web-a-debug-26.11");
        h.file(&format!("{debug}/nixos-version"), "26.11\n")
            .link(&format!("{t}/specialisation/debug"), &debug);
    }
    t
}

/// A toplevel without a marker: a regular file holding `text`, an `etc` link and a package.
pub fn unknown(host: &mut Host, seed: &str, text: &str) -> String {
    let etc = store_path(seed, "etc");
    host.file(&format!("{etc}/etc/motd"), text);
    let tool = store_path(seed, "tool-1.0");
    host.file(&format!("{tool}/bin/tool"), "\0");
    let t = store_path(seed, "custom-system");
    host.file(&format!("{t}/hello"), text)
        .link(&format!("{t}/etc"), &format!("{etc}/etc"))
        .link(&format!("{t}/tool"), &tool);
    t
}

/// A nix-darwin generation: launchd plists under `Library` and `user/Library` (their
/// `StartInterval` is `interval`), an `etc` file, and `Applications`, which is not followed.
pub fn nix_darwin(host: &mut Host, seed: &str, interval: &str) -> String {
    let plist = |label: &str| {
        format!(
            "<plist><dict><key>Label</key><string>{label}</string><key>StartInterval</key><integer>{interval}</integer></dict></plist>\n"
        )
    };
    let daemons = store_path(seed, "launchd");
    host.file(
        &format!("{daemons}/Library/LaunchDaemons/org.nixos.gc.plist"),
        plist("org.nixos.gc"),
    );
    let agents = store_path(seed, "user-launchd");
    host.file(
        &format!("{agents}/user/Library/LaunchAgents/org.example.sync.plist"),
        plist("org.example.sync"),
    );
    let apps = store_path(seed, "system-applications");
    host.file(&format!("{apps}/Applications/App.app/Info.plist"), seed);
    let etc = store_path(seed, "etc");
    host.file(&format!("{etc}/etc/zshrc"), format!("# {seed}\n"));
    let t = store_path(seed, "darwin-system-26.11.ff0e21a");
    host.file(&format!("{t}/darwin-version"), "26.11.ff0e21a\n")
        .file(&format!("{t}/darwin-version.json"), seed)
        .link(&format!("{t}/etc"), &format!("{etc}/etc"))
        .link(
            &format!("{t}/Library/LaunchDaemons"),
            &format!("{daemons}/Library/LaunchDaemons"),
        )
        .link(
            &format!("{t}/user/Library/LaunchAgents"),
            &format!("{agents}/user/Library/LaunchAgents"),
        )
        .link(
            &format!("{t}/Applications"),
            &format!("{apps}/Applications"),
        );
    t
}

/// A home-manager generation: a git config naming `email` under `home-files`, a launchd agent,
/// and `home-path`, which is not followed.
pub fn home_manager(host: &mut Host, seed: &str, email: &str) -> String {
    let gitconfig = store_path(seed, "hm_gitconfig");
    host.file(&gitconfig, format!("[user]\n\temail = {email}\n"));
    let files = store_path(seed, "home-manager-files");
    host.link(&format!("{files}/.config/git/config"), &gitconfig);
    let agents = store_path(seed, "home-manager-agents");
    host.file(
        &format!("{agents}/org.example.agent.plist"),
        format!("<plist>{seed}</plist>\n"),
    );
    let path = store_path(seed, "home-manager-path");
    host.file(&format!("{path}/bin/tool"), seed);
    let t = store_path(seed, "home-manager-generation");
    host.file(&format!("{t}/hm-version"), "26.11\n")
        .link(&format!("{t}/home-files"), &files)
        .link(&format!("{t}/LaunchAgents"), &agents)
        .link(&format!("{t}/home-path"), &path);
    t
}

/// [`CURRENT_SYSTEM`] pointing at `toplevel`.
pub fn current_system(host: &mut Host, toplevel: &str) {
    host.link(CURRENT_SYSTEM, toplevel);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cksum_matches_posix() {
        // Values from `cksum` on macOS and GNU coreutils.
        assert_eq!(cksum(b""), 4294967295);
        assert_eq!(cksum(b"x\n"), 2192966820);
    }

    #[test]
    fn store_paths_are_stable_and_valid() {
        let p = store_path("old", "etc");
        assert_eq!(p, store_path("old", "etc"));
        assert_ne!(p, store_path("new", "etc"));
        assert_eq!(store_refs(&p).len(), 1);
    }

    #[test]
    fn resolves_links_through_parents() {
        let mut host = Host::new();
        host.file("/nix/store/a-x/etc/hosts", "h\n")
            .link("/nix/store/b-y/etc", "/nix/store/a-x/etc")
            .link("/run/current-system", "/nix/store/b-y")
            .link("/nix/store/b-y/rel", "etc/hosts");
        assert_eq!(
            host.resolve("/run/current-system/etc/hosts").as_deref(),
            Some("/nix/store/a-x/etc/hosts")
        );
        assert_eq!(
            host.resolve("/nix/store/b-y/rel").as_deref(),
            Some("/nix/store/a-x/etc/hosts")
        );
        assert_eq!(host.resolve("/nix/store/b-y/missing"), None);
        let (canonical, node) = host.lstat("/run/current-system/etc").unwrap();
        assert_eq!(canonical, "/nix/store/b-y/etc");
        assert!(matches!(node, Node::Link(_)));
    }

    /// The real script on this machine's tools against the same tree on disk: the fixture's
    /// answers parse the same. On macOS the system's BSD tools come first, as on a target. The
    /// probe only shows that both refuse a toplevel outside the store: a temporary directory
    /// cannot be in it.
    #[test]
    fn answers_like_the_real_script() {
        use crate::exec::Command;
        use crate::fetch::{content, listing, probe, references};
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let r = |rel: &str| format!("{}/{rel}", root.display());
        let mut host = Host::new();
        let files: [(&str, &[u8]); 8] = [
            ("top/nixos-version", b"26.05\n"),
            ("top/kernel-params", b"quiet"),
            ("etc/etc/hosts", b"127.0.0.1 localhost\n"),
            ("etc/etc/sub dir/a", b"x\n"),
            ("pkg/bin/app", b"\0binary"),
            ("pkg/share/doc", b"d"),
            ("small/bin/run", b"#!/bin/sh\n"),
            ("small/bin/odd\\x41", b"x"),
        ];
        for (rel, data) in files {
            std::fs::create_dir_all(std::path::Path::new(&r(rel)).parent().unwrap()).unwrap();
            std::fs::write(r(rel), data).unwrap();
            host.file(&r(rel), data);
        }
        // An extended attribute makes macOS tar add a `._run` entry unless told not to.
        if cfg!(target_os = "macos") {
            let status = std::process::Command::new("/usr/bin/xattr")
                .args(["-w", "user.scry", "x", &r("small/bin/run")])
                .status()
                .unwrap();
            assert!(status.success());
        }
        let links = [
            ("top/etc", r("etc/etc")),
            (
                "top/kernel",
                "/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-linux-6.12/bzImage".into(),
            ),
            ("top/specialisation/debug", "/x".into()),
            ("etc/etc/mtab", "/proc/mounts".into()),
            ("current", r("top")),
        ];
        for (rel, target) in &links {
            std::fs::create_dir_all(std::path::Path::new(&r(rel)).parent().unwrap()).unwrap();
            symlink(target, r(rel)).unwrap();
            host.link(&r(rel), target);
        }
        let bin = r("bin");
        std::fs::create_dir(&bin).unwrap();
        std::fs::write(format!("{bin}/nix"), "#!/bin/sh\necho {}\n").unwrap();
        std::fs::set_permissions(
            format!("{bin}/nix"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();

        let real = |command: &Command| {
            let system = if cfg!(target_os = "macos") {
                "/usr/bin:/bin:"
            } else {
                ""
            };
            let path = format!("{bin}:{system}{}", std::env::var("PATH").unwrap());
            let mut child = std::process::Command::new(&command.program)
                .args(&command.args)
                .env("PATH", path)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(&command.stdin)
                .unwrap();
            let out = child.wait_with_output().unwrap();
            Output {
                code: out.status.code(),
                stdout: out.stdout,
                stderr: out.stderr,
            }
        };
        let fake = |command: &Command| host.answer(&command.args, &command.stdin);
        let parsed = |listing: &[u8]| -> BTreeSet<(String, String)> {
            String::from_utf8_lossy(listing)
                .lines()
                .map(|l| {
                    let (path, listed) = crate::tree::parse_line(l).expect(l);
                    (path.to_owned(), format!("{listed:?}"))
                })
                .collect()
        };

        let command = probe::command(&[probe::Request(Some(r("current")))]);
        let (a, b) = (real(&command), fake(&command));
        let (a, b) = (
            probe::parse(&Machine::Local, &a).unwrap(),
            probe::parse(&Machine::Local, &b).unwrap(),
        );
        assert_eq!(a, b);
        assert_eq!(a.sides[0].toplevel, None);

        let roots = BTreeSet::from([r("etc/etc"), r("missing"), r("top/etc")]);
        let command = listing::command(&roots);
        let (a, b) = (real(&command), fake(&command));
        assert_eq!(parsed(&a.stdout), parsed(&b.stdout));

        let paths = [r("etc/etc/hosts"), r("top/etc/sub dir/a")];
        for gzip in [false, true] {
            let command = content::command(&paths, gzip);
            let (a, b) = (real(&command), fake(&command));
            assert_eq!(
                content::unpack(&a.stdout, gzip).unwrap(),
                content::unpack(&b.stdout, gzip).unwrap()
            );
        }

        use references::Take;
        let roots = BTreeMap::from([
            (r("etc/etc"), Take::Whole),
            (r("pkg"), Take::File),
            (r("small"), Take::Whole),
            (r("top"), Take::Bin),
            (r("top/kernel-params"), Take::File),
            (r("top/nixos-version"), Take::Bin),
        ]);
        let command = references::command(&roots, true, 10);
        let (a, b) = (real(&command), fake(&command));
        let split = |out: &Output| {
            let at = crate::fetch::find(&out.stdout, b"::scry tar\n").unwrap();
            let archive = content::unpack(&out.stdout[at + 11..], true).unwrap();
            (parsed(&out.stdout[..at]), archive)
        };
        let (a, b) = (split(&a), split(&b));
        assert_eq!(a, b);
        // Of the directories only `small/bin/run`: `pkg` is taken as a file, `top` has no
        // `bin/`, and the backslash name stays out; then the file `kernel-params`.
        let mut archived: Vec<&String> = a.1.keys().collect();
        archived.sort();
        assert_eq!(archived, [&r("small/bin/run"), &r("top/kernel-params")]);
        assert!(a.0.contains(&(r("small"), "Directory".to_owned())));
    }
}
