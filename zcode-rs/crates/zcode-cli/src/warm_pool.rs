//! Warm app-server/agent-server pool (batch-3, assignment A).
//!
//! T3 measured app-server time-to-ready at 378 ms median (~375 MB RSS), 100%
//! Node bundle boot. This module hides that boot for second-and-later
//! protocol-server invocations: a detached daemon pre-spawns ONE node
//! app-server child whose stdin/stdout are the child end of a unix
//! socketpair; when a real client arrives and its environment matches, the
//! daemon hands the already-booted child to it by relaying
//! client-stdio <-> unix-socket <-> socketpair.
//!
//! Wire protocol on `$TMPDIR/zcode-warm-pool-$UID.sock` (all frames tiny,
//! fixed layout; the relay that follows is raw byte-transparent):
//!
//! ```text
//! client -> daemon: [kind u8 ('a'|'g')] [cwd_len u16 LE] [cwd] [env_fp u64 LE]
//! daemon -> client: b'W' (warm handoff follows) | b'N' (not pooled; client direct-execs)
//! ```
//!
//! Served only when the invocation is EXACTLY `zcode app-server` (or
//! `agent-server`) with no extra flags; anything else (and any mismatch of
//! cwd / env fingerprint / warm-kind) falls back to `exec_fallback`, which is
//! byte-identical to the pre-pool behavior. The daemon replenishes its warm
//! child immediately after each handoff (spawn is ~2 ms; the 300 ms bundle
//! boot proceeds asynchronously in the child) and exits after IDLE_SECS with
//! no active session.
//!
//! Daemon trigger: the client spawns `zcode __warm-pool-daemon` semantics via
//! a sibling invocation of this same binary with argv `app-server` plus env
//! `ZCODE_WARM_POOL_DAEMON=1` (run.rs routes bare app-server here; the env
//! marker is checked before any parsing). The literal argv[0]
//! `__warm-pool-daemon` is honored too if a future router passes it.
//!
//! Env: `ZCODE_WARM_POOL` (default on; "0"/"false"/"off" disables),
//! `ZCODE_WARM_POOL_IDLE_SECS` (default 300), `ZCODE_WARM_POOL_SOCK`
//! (socket path override for tests).

#[cfg(unix)]
mod imp {
    use std::fs::{OpenOptions, Permissions};
    use std::io::{Read, Write};
    use std::net::Shutdown;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::os::unix::process::CommandExt;
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    const KIND_APP: u8 = b'a';
    const KIND_AGENT: u8 = b'g';
    /// A warm child younger than this is still mid-boot; serving it would save
    /// nothing over a direct exec, so the client is told to take the cold path.
    const READY_MIN: Duration = Duration::from_millis(250);
    const HANDSHAKE_TIMEOUT_MS: u64 = 1000;
    const DEFAULT_IDLE_SECS: u64 = 300;
    const REPLY_WARM: u8 = b'W';
    const REPLY_COLD: u8 = b'N';

    struct WarmSlot {
        kind: u8,
        child: Child,
        /// Daemon's ends of two OS pipes (same handle types a direct exec
        /// uses): `feed` writes child stdin, `collect` drains child stdout.
        /// Dropping `feed` at session end EOFs the child's stdin, exactly
        /// like a direct-exec client closing its pipe.
        feed: std::io::PipeWriter,
        collect: std::io::PipeReader,
        born: Instant,
        cwd: String,
        fp: u64,
    }

    struct Shared {
        slot: Mutex<Option<WarmSlot>>,
        active: AtomicUsize,
        last_activity: Mutex<Instant>,
    }

    // ---------------------------------------------------------------- entry

    pub fn run(argv: &[String]) -> i32 {
        // Daemon self-identification, before ANY parsing (spec: hidden
        // subcommand arrives as argv[0]; the shipped trigger is the env
        // marker because run.rs is not ours to edit).
        let is_daemon = argv.first().map(|s| s.as_str()) == Some("__warm-pool-daemon")
            || std::env::var("ZCODE_WARM_POOL_DAEMON").as_deref() == Ok("1");
        if is_daemon {
            return daemon_main();
        }
        let kind = match argv.first().map(|s| s.as_str()) {
            Some("app-server") => KIND_APP,
            Some("agent-server") => KIND_AGENT,
            _ => return crate::fallback::exec_fallback(argv),
        };
        // Debug utility for tests: print this process's env fingerprint.
        if std::env::var_os("ZCODE_WARM_POOL_DUMP_FP").is_some() {
            println!("{:016x}", env_fingerprint());
            return 0;
        }
        if pool_disabled() || argv.len() != 1 {
            // Flagged invocations (e.g. `app-server --surface desktop`) keep
            // the exact legacy byte path; the pool only serves bare calls.
            return crate::fallback::exec_fallback(argv);
        }
        match try_pool(kind) {
            Pool::Warm(sock) => client_relay(sock),
            Pool::Cold => crate::fallback::exec_fallback(argv),
            Pool::NoDaemon => {
                spawn_daemon();
                crate::fallback::exec_fallback(argv)
            }
        }
    }

    fn pool_disabled() -> bool {
        matches!(
            std::env::var("ZCODE_WARM_POOL").as_deref(),
            Ok("0") | Ok("false") | Ok("off")
        )
    }

    // --------------------------------------------------------------- client

    enum Pool {
        Warm(UnixStream),
        Cold,
        NoDaemon,
    }

    fn try_pool(kind: u8) -> Pool {
        let Some(path) = socket_path() else {
            return Pool::NoDaemon;
        };
        let Ok(mut sock) = UnixStream::connect(&path) else {
            return Pool::NoDaemon;
        };
        let _ = sock.set_read_timeout(Some(Duration::from_millis(HANDSHAKE_TIMEOUT_MS)));
        let _ = sock.set_write_timeout(Some(Duration::from_millis(HANDSHAKE_TIMEOUT_MS)));
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        if cwd.len() > 4096 {
            return Pool::Cold;
        }
        let mut msg = Vec::with_capacity(3 + cwd.len() + 8);
        msg.push(kind);
        msg.extend_from_slice(&(cwd.len() as u16).to_le_bytes());
        msg.extend_from_slice(cwd.as_bytes());
        msg.extend_from_slice(&env_fingerprint().to_le_bytes());
        if sock.write_all(&msg).is_err() {
            return Pool::Cold;
        }
        let mut reply = [0u8; 1];
        match sock.read(&mut reply) {
            Ok(1) if reply[0] == REPLY_WARM => Pool::Warm(sock),
            Ok(1) => Pool::Cold,
            _ => Pool::Cold,
        }
    }

    /// Proxy our stdio over the socket until EOF, then exit like the child
    /// would: clean EOF = 0 (TS `runZCodeProtocolCommand` returns 0 after a
    /// stdin-driven close), abrupt socket error = 1.
    fn client_relay(sock: UnixStream) -> i32 {
        let Ok(sock_w) = sock.try_clone() else {
            return 1;
        };
        let up = std::thread::spawn(move || {
            let mut sock_w = sock_w;
            {
                let mut si = std::io::stdin().lock();
                pump(&mut si, &mut sock_w);
            }
            let _ = sock_w.shutdown(Shutdown::Write);
        });
        let mut clean = true;
        {
            let mut so = std::io::stdout().lock();
            let mut sock = sock;
            let mut buf = [0u8; 16384];
            loop {
                match sock.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if so.write_all(&buf[..n]).is_err() || so.flush().is_err() {
                            clean = false;
                            break;
                        }
                    }
                    Err(_) => {
                        clean = false;
                        break;
                    }
                }
            }
        }
        let _ = up.join();
        if clean {
            0
        } else {
            1
        }
    }

    fn pump(r: &mut dyn Read, w: &mut dyn Write) {
        let mut buf = [0u8; 16384];
        loop {
            match r.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    if w.write_all(&buf[..n]).is_err() {
                        return;
                    }
                }
            }
        }
    }

    /// Spawn the detached daemon (best effort). It inherits our cwd/env so its
    /// first warm child matches the most common future client. LANG/LC_ALL are
    /// stripped so run.rs's zh-locale intercept cannot shadow the warm_pool
    /// route, and passed through side vars for re-application on node children.
    fn spawn_daemon() {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let mut cmd = Command::new(exe);
        cmd.arg("app-server")
            .env("ZCODE_WARM_POOL_DAEMON", "1")
            .env_remove("LANG")
            .env_remove("LC_ALL");
        if let Ok(l) = std::env::var("LANG") {
            cmd.env("ZCODE_WARM_POOL_LANG", l);
        }
        if let Ok(l) = std::env::var("LC_ALL") {
            cmd.env("ZCODE_WARM_POOL_LC_ALL", l);
        }
        cmd.stdin(Stdio::null()).stdout(Stdio::null());
        let log = socket_path()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .map(|d| d.join("zcode-warm-pool-daemon.log"))
            .and_then(|p| OpenOptions::new().create(true).append(true).open(p).ok());
        match log {
            Some(f) => {
                cmd.stderr(Stdio::from(f));
            }
            None => {
                cmd.stderr(Stdio::null());
            }
        }
        // Own process group: a terminal Ctrl-C on the client must not kill the
        // daemon; when the client exits the daemon is reparented to init.
        cmd.process_group(0);
        let _ = cmd.spawn();
    }

    // --------------------------------------------------------------- daemon

    fn daemon_main() -> i32 {
        let Some(path) = socket_path() else {
            return 1;
        };
        let Some(listener) = bind_socket(&path) else {
            // Another live daemon owns the socket (or the path is unusable);
            // exit quietly, clients direct-exec in the meantime.
            return 0;
        };
        let _ = std::fs::set_permissions(&path, Permissions::from_mode(0o600));
        let shared = Arc::new(Shared {
            slot: Mutex::new(None),
            active: AtomicUsize::new(0),
            last_activity: Mutex::new(Instant::now()),
        });
        // The daemon is always seeded as an app-server (that is how it is
        // invoked); agent-server demand lazily re-seeds the slot. The seed is
        // delayed ~1s so the seeding client's own (direct-exec) Node boot does
        // not contend with the warm child's boot: node aborts a session whose
        // stdin EOF is not drained within 100ms of boot (PROTOCOL_INPUT_DRAIN_MS),
        // and parallel boots were flipping that race for the seeding client.
        {
            let shared2 = Arc::clone(&shared);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(1000));
                let empty = shared2.slot.lock().unwrap().is_none();
                if !empty {
                    return;
                }
                let warm = spawn_warm(KIND_APP);
                if std::env::var_os("ZCODE_WARM_POOL_DEBUG").is_some() {
                    eprintln!("zcode-warm-pool[daemon]: initial spawn_warm ok={}", warm.is_some());
                }
                if let Some(s) = warm {
                    *shared2.slot.lock().unwrap() = Some(s);
                }
            });
        }
        let idle = idle_duration();
        let _ = listener.set_nonblocking(true);
        loop {
            match listener.accept() {
                Ok((sock, _)) => {
                    *shared.last_activity.lock().unwrap() = Instant::now();
                    serve_conn(&shared, sock);
                }
                Err(_) => {}
            }
            {
                let mut slot = shared.slot.lock().unwrap();
                if let Some(s) = slot.as_mut() {
                    if matches!(s.child.try_wait(), Ok(Some(_))) {
                        if std::env::var_os("ZCODE_WARM_POOL_DEBUG").is_some() {
                            eprintln!(
                                "zcode-warm-pool[daemon]: warm child exited: {:?}",
                                s.child.try_wait()
                            );
                        }
                        *slot = None; // warm child died pre-handoff; lazy re-seed
                    }
                }
            }
            if shared.active.load(Ordering::SeqCst) == 0
                && shared.last_activity.lock().unwrap().elapsed() >= idle
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        if let Some(mut s) = shared.slot.lock().unwrap().take() {
            let _ = s.child.kill();
            let _ = s.child.wait();
        }
        let _ = std::fs::remove_file(&path);
        0
    }

    fn bind_socket(path: &std::path::Path) -> Option<UnixListener> {
        match UnixListener::bind(path) {
            Ok(l) => Some(l),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                // Alive daemon? Then we are a racing duplicate: stand down.
                if UnixStream::connect(path).is_ok() {
                    return None;
                }
                // Stale socket from a dead daemon: unlink and rebind.
                let _ = std::fs::remove_file(path);
                UnixListener::bind(path).ok()
            }
            Err(_) => None,
        }
    }

    fn serve_conn(shared: &Arc<Shared>, mut sock: UnixStream) {
        // On BSD/macOS accept() inherits the listener's O_NONBLOCK; the relay
        // pumps and handshake need blocking semantics.
        let _ = sock.set_nonblocking(false);
        let _ = sock.set_read_timeout(Some(Duration::from_millis(HANDSHAKE_TIMEOUT_MS)));
        let _ = sock.set_write_timeout(Some(Duration::from_millis(HANDSHAKE_TIMEOUT_MS)));
        let Some((kind, cwd, fp)) = read_handshake(&mut sock) else {
            return;
        };
        let mut handoff: Option<WarmSlot> = None;
        {
            let mut slot = shared.slot.lock().unwrap();
            if let Some(s) = slot.as_mut() {
                if matches!(s.child.try_wait(), Ok(Some(_))) {
                    *slot = None;
                }
            }
            let warmable = slot.as_ref().is_some_and(|s| {
                s.kind == kind && s.cwd == cwd && s.fp == fp && s.born.elapsed() >= READY_MIN
            });
            if std::env::var_os("ZCODE_WARM_POOL_DEBUG").is_some() {
                let mut keys: Vec<String> = std::env::vars().map(|(k, _)| k).collect();
                keys.sort();
                eprintln!("zcode-warm-pool[daemon]: env keys: {keys:?}");
                match slot.as_ref() {
                    Some(s) => eprintln!(
                        "zcode-warm-pool[daemon]: handshake kind={kind} cwd={cwd} fp={fp:016x} | slot kind={} cwd={} fp={:016x} age={:?} warmable={warmable}",
                        s.kind, s.cwd, s.fp, s.born.elapsed()
                    ),
                    None => eprintln!(
                        "zcode-warm-pool[daemon]: handshake kind={kind} cwd={cwd} fp={fp:016x} | slot EMPTY"
                    ),
                }
            }
            if warmable {
                if sock.write_all(&[REPLY_WARM]).is_ok() {
                    handoff = slot.take();
                }
            } else {
                let _ = sock.write_all(&[REPLY_COLD]);
            }
        }
        match handoff {
            Some(entry) => {
                shared.active.fetch_add(1, Ordering::SeqCst);
                let kind = entry.kind;
                handoff_to_client(entry, sock, Arc::clone(shared));
                // Replenish right away: spawn returns in ~ms, the bundle boot
                // proceeds asynchronously inside the new child.
                if let Some(n) = spawn_warm(kind) {
                    *shared.slot.lock().unwrap() = Some(n);
                    *shared.last_activity.lock().unwrap() = Instant::now();
                }
            }
            None => {
                // Lazy re-seed (covers pre-handoff death and first agent-server).
                let empty = shared.slot.lock().unwrap().is_none();
                if empty {
                    if let Some(n) = spawn_warm(kind) {
                        *shared.slot.lock().unwrap() = Some(n);
                        *shared.last_activity.lock().unwrap() = Instant::now();
                    }
                }
            }
        }
    }

    fn handoff_to_client(entry: WarmSlot, client: UnixStream, shared: Arc<Shared>) {
        let WarmSlot {
            mut child,
            feed,
            collect,
            ..
        } = entry;
        let Ok(client_r) = client.try_clone() else {
            let _ = child.kill();
            let _ = child.wait();
            shared.active.fetch_sub(1, Ordering::SeqCst);
            return;
        };
        // The handshake timeout must not bound the session: clear it so the
        // relays block for the life of the connection (idle pauses included).
        let _ = client_r.set_read_timeout(None);
        let _ = client_r.set_write_timeout(None);
        let _ = client.set_read_timeout(None);
        let _ = client.set_write_timeout(None);
        // client stdin -> child stdin; dropping `feed` at client EOF tells
        // the child its stdin closed (pipe EOF, identical to a direct exec).
        let t0 = Instant::now();
        std::thread::spawn(move || {
            let mut feed = feed;
            let mut client_r = client_r;
            pump(&mut client_r, &mut feed);
            if std::env::var_os("ZCODE_WARM_POOL_DEBUG").is_some() {
                eprintln!("zcode-warm-pool[daemon]: uplink relay ended at +{:?}", t0.elapsed());
            }
            drop(feed);
        });
        // child stdout -> client stdout; when the child exits the client sees
        // EOF and exits 0, mirroring a direct exec.
        std::thread::spawn(move || {
            let mut collect = collect;
            let mut client = client;
            pump(&mut collect, &mut client);
            if std::env::var_os("ZCODE_WARM_POOL_DEBUG").is_some() {
                eprintln!("zcode-warm-pool[daemon]: downlink relay ended at +{:?}", t0.elapsed());
            }
            let _ = client.shutdown(Shutdown::Both);
        });
        // Reap so no zombie; decrement the stay-alive counter.
        std::thread::spawn(move || {
            let status = child.wait();
            if std::env::var_os("ZCODE_WARM_POOL_DEBUG").is_some() {
                eprintln!("zcode-warm-pool[daemon]: handed-off child exited: {status:?}");
            }
            shared.active.fetch_sub(1, Ordering::SeqCst);
        });
    }

    fn read_handshake(sock: &mut UnixStream) -> Option<(u8, String, u64)> {
        let mut hdr = [0u8; 3];
        sock.read_exact(&mut hdr).ok()?;
        let kind = hdr[0];
        if kind != KIND_APP && kind != KIND_AGENT {
            return None;
        }
        let cwd_len = u16::from_le_bytes([hdr[1], hdr[2]]) as usize;
        if cwd_len > 4096 {
            return None;
        }
        let mut cwd_buf = vec![0u8; cwd_len];
        sock.read_exact(&mut cwd_buf).ok()?;
        let mut fp_buf = [0u8; 8];
        sock.read_exact(&mut fp_buf).ok()?;
        Some((
            kind,
            String::from_utf8_lossy(&cwd_buf).into_owned(),
            u64::from_le_bytes(fp_buf),
        ))
    }

    /// Spawn the pre-warmed node protocol server. Its stdin/stdout are the
    /// child end of a fresh socketpair, so the daemon can relay it to any
    /// future client without fd passing. Env mirrors `exec_fallback`
    /// (including the NODE_COMPILE_CACHE default) minus pool plumbing.
    fn spawn_warm(kind: u8) -> Option<WarmSlot> {
        let bundle = crate::fallback::node_bundle_path();
        let node = std::env::var("ZCODE_NODE").unwrap_or_else(|_| "node".to_string());
        let (stdin_child, feed) = std::io::pipe().ok()?;
        let (collect, stdout_child) = std::io::pipe().ok()?;
        let mut cmd = Command::new(&node);
        cmd.arg(&bundle)
            .arg(if kind == KIND_APP { "app-server" } else { "agent-server" });
        if std::env::var_os("NODE_COMPILE_CACHE").is_none() {
            let port_dir = std::env::var("ZCODE_PORT_DIR")
                .unwrap_or_else(|_| env!("ZCODE_PORT_DIR_DEFAULT").to_string());
            cmd.env(
                "NODE_COMPILE_CACHE",
                PathBuf::from(port_dir).join(".node-compile-cache"),
            );
        }
        cmd.env_remove("ZCODE_WARM_POOL_DAEMON");
        // Locale stripped at daemon spawn is restored here so node children
        // behave exactly as they would under a direct exec.
        if std::env::var_os("LANG").is_none() {
            if let Some(l) = std::env::var_os("ZCODE_WARM_POOL_LANG") {
                cmd.env("LANG", l);
            }
        }
        if std::env::var_os("LC_ALL").is_none() {
            if let Some(l) = std::env::var_os("ZCODE_WARM_POOL_LC_ALL") {
                cmd.env("LC_ALL", l);
            }
        }
        cmd.stdin(Stdio::from(stdin_child))
            .stdout(Stdio::from(stdout_child))
            .stderr(Stdio::inherit()); // daemon stderr = pool log file
        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                if std::env::var_os("ZCODE_WARM_POOL_DEBUG").is_some() {
                    eprintln!("zcode-warm-pool[daemon]: spawn_warm failed: {e}");
                }
                return None;
            }
        };
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        Some(WarmSlot {
            kind,
            child,
            feed,
            collect,
            born: Instant::now(),
            cwd,
            fp: env_fingerprint(),
        })
    }

    // ------------------------------------------------------------- plumbing

    fn idle_duration() -> Duration {
        let secs = std::env::var("ZCODE_WARM_POOL_IDLE_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_IDLE_SECS)
            .max(10);
        Duration::from_secs(secs)
    }

    fn socket_path() -> Option<PathBuf> {
        if let Ok(p) = std::env::var("ZCODE_WARM_POOL_SOCK") {
            if !p.is_empty() {
                return Some(PathBuf::from(p));
            }
        }
        let dir = std::env::var("TMPDIR")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/tmp".to_string());
        let mut p = PathBuf::from(dir);
        p.push(format!("zcode-warm-pool{}.sock", uid_suffix()));
        Some(p)
    }

    /// `$UID` when exported; otherwise `id -u` on platforms with a shared
    /// /tmp. On macOS $TMPDIR is already per-user, so the suffix is skipped
    /// there unless UID is explicitly set (keeps every client connect cheap).
    fn uid_suffix() -> String {
        static UID: OnceLock<String> = OnceLock::new();
        UID.get_or_init(|| {
            if let Ok(u) = std::env::var("UID") {
                let u = u.trim();
                if !u.is_empty() && u.bytes().all(|b| b.is_ascii_digit()) {
                    return format!("-{u}");
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                if let Ok(out) = Command::new("id").arg("-u").output() {
                    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
                        return format!("-{s}");
                    }
                }
            }
            String::new()
        })
        .clone()
    }

    /// FNV-1a over the sorted effective environment (same adjustments
    /// `spawn_warm` applies), excluding pool plumbing and process noise.
    /// A client whose environment differs from the daemon's is served cold —
    /// wrong-env sessions are worse than slow ones.
    fn env_fingerprint() -> u64 {
        let mut vars: Vec<(String, String)> = std::env::vars()
            .filter(|(k, _)| !is_noise_key(k))
            .collect();
        let has = |vars: &[(String, String)], k: &str| vars.iter().any(|(key, _)| key == k);
        if !has(&vars, "LANG") {
            if let Ok(l) = std::env::var("ZCODE_WARM_POOL_LANG") {
                vars.push(("LANG".to_string(), l));
            }
        }
        if !has(&vars, "LC_ALL") {
            if let Ok(l) = std::env::var("ZCODE_WARM_POOL_LC_ALL") {
                vars.push(("LC_ALL".to_string(), l));
            }
        }
        vars.sort();
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for (k, v) in vars {
            for b in k.bytes().chain(b"=".iter().copied()).chain(v.bytes()) {
                h ^= b as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
            h ^= 0xff;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }

    fn is_noise_key(k: &str) -> bool {
        matches!(
            k,
            "_"
                | "PWD"
                | "OLDPWD"
                | "SHLVL"
                | "NODE_COMPILE_CACHE"
                | "ZCODE_WARM_POOL"
                | "ZCODE_WARM_POOL_DAEMON"
                | "ZCODE_WARM_POOL_DUMP_FP"
                | "ZCODE_WARM_POOL_IDLE_SECS"
                | "ZCODE_WARM_POOL_SOCK"
                | "ZCODE_WARM_POOL_LANG"
                | "ZCODE_WARM_POOL_LC_ALL"
        )
    }
}

#[cfg(unix)]
pub fn run(argv: &[String]) -> i32 {
    imp::run(argv)
}

#[cfg(not(unix))]
pub fn run(argv: &[String]) -> i32 {
    // Warm pooling is a unix-socket design; other platforms keep the exact
    // legacy fallback path.
    crate::fallback::exec_fallback(argv)
}
