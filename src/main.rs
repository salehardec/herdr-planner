//! Herdr Planner: a standalone dashboard of the herdr-projects board.
//!
//! One binary: the HTTP(S) server, the board poller and the embedded page.
//! It does not need herdr or herdr-gpui to run.

mod api;
mod app;
mod attachments;
mod auth;
mod board;
mod config;
mod coordinator;
mod db;
mod events;
mod gitlag;
mod history;
mod memory;
mod memory_api;
mod memory_guard;
mod models;
mod search;
mod server;
mod ship;
mod station;
mod subagents;
mod taskmeta;
mod translate;
mod watch;
#[cfg(test)]
mod testutil;
mod theme;
mod universe;
mod usage;

use app::{refresh, Shared};
use config::AppConfig;
use std::io::Read;
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Duration;

fn help() {
    println!(
        "herdr-planner [--listen ADDR] [--port N] [--data DIR]

  Запускает дашборд доски herdr-projects на http://127.0.0.1:7117 (или https,
  когда в конфиге есть tls_cert и tls_key). Без password_hash слушает только
  loopback; чтобы открыть с другой машины, задайте пароль:
      scripts/install.sh password --listen 0.0.0.0

  Файл настроек: ~/.config/herdr-planner/config.toml
  Состояние (журнал, чат): ~/.local/state/herdr-planner/

  Служебные команды:
      herdr-planner hash-password        прочитать пароль из stdin и напечатать Argon2id-хеш
      herdr-planner set-auth [--user U] [--listen A]
                                         записать хеш из stdin (и логин/адрес) в конфиг
      herdr-planner set-tls --cert F --key F
                                         записать пути сертификата и ключа в конфиг
"
    );
}

fn read_stdin_trimmed() -> Result<String, String> {
    let mut s = String::new();
    std::io::stdin()
        .read_to_string(&mut s)
        .map_err(|e| e.to_string())?;
    Ok(s.trim_end_matches(['\r', '\n']).to_string())
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn socket_addr(listen: &str, port: u16) -> Result<SocketAddr, String> {
    let host = listen.trim();
    let hostport = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    hostport
        .to_socket_addrs()
        .map_err(|e| format!("{hostport}: {e}"))?
        .next()
        .ok_or_else(|| format!("{hostport}: no address"))
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        help();
        return;
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("herdr-planner {}", app::VERSION);
        return;
    }
    if args.iter().any(|a| a == "hash-password") {
        match read_stdin_trimmed().and_then(|p| {
            if p.is_empty() {
                Err("empty password".to_string())
            } else {
                config::hash_password(&p)
            }
        }) {
            Ok(hash) => println!("{hash}"),
            Err(e) => {
                eprintln!("herdr-planner hash-password: {e}");
                std::process::exit(2);
            }
        }
        return;
    }
    if args.iter().any(|a| a == "set-auth") {
        let hash = match read_stdin_trimmed() {
            Ok(h) if h.starts_with("$argon2") || h.starts_with("$2") => h,
            Ok(_) => {
                eprintln!("herdr-planner set-auth: expected an Argon2 or bcrypt hash on stdin");
                std::process::exit(2);
            }
            Err(e) => {
                eprintln!("herdr-planner set-auth: {e}");
                std::process::exit(2);
            }
        };
        let cfg = AppConfig::load(&args);
        let user = flag(&args, "--user").unwrap_or(cfg.user);
        let listen = flag(&args, "--listen");
        match config::set_auth(&cfg.config_file, &user, &hash, listen.as_deref()) {
            Ok(()) => println!(
                "herdr-planner: login for «{user}» written to {}",
                cfg.config_file.display()
            ),
            Err(e) => {
                eprintln!("herdr-planner set-auth: {e}");
                std::process::exit(2);
            }
        }
        return;
    }
    if args.iter().any(|a| a == "set-tls") {
        let cfg = AppConfig::load(&args);
        let (Some(cert), Some(key)) = (flag(&args, "--cert"), flag(&args, "--key")) else {
            eprintln!("herdr-planner set-tls: --cert and --key are required");
            std::process::exit(2);
        };
        match config::set_tls(&cfg.config_file, &cert, &key) {
            Ok(()) => println!(
                "herdr-planner: TLS certificate written to {}",
                cfg.config_file.display()
            ),
            Err(e) => {
                eprintln!("herdr-planner set-tls: {e}");
                std::process::exit(2);
            }
        }
        return;
    }

    let cfg = AppConfig::load(&args);
    if !cfg.auth_required() && !cfg.is_loopback() {
        eprintln!(
            "herdr-planner: {} is not a loopback address, but no password_hash is set.\n\
             Set one with `scripts/install.sh password --listen {}` (or keep 127.0.0.1).",
            cfg.listen, cfg.listen
        );
        std::process::exit(1);
    }
    if cfg.auth_required() && !cfg.is_loopback() && cfg.tls().is_none() {
        eprintln!(
            "herdr-planner: warning: listening on {} over plain HTTP. The session cookie \
             is Secure, so a remote browser will not keep it; configure tls_cert/tls_key \
             or put a TLS proxy (or Tailscale Serve) in front.",
            cfg.listen
        );
    }
    if let Some((cert, key)) = cfg.tls() {
        if !cert.exists() || !key.exists() {
            eprintln!(
                "herdr-planner: TLS files missing: {} / {}",
                cert.display(),
                key.display()
            );
            std::process::exit(1);
        }
    }

    let addr = match socket_addr(&cfg.listen, cfg.port) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("herdr-planner: {e}");
            std::process::exit(1);
        }
    };
    let app = app::App::new(cfg.clone());
    // The first read happens before the socket opens: the page never starts empty.
    refresh(&app).await;
    let _watch = watch::start(app.clone());
    spawn_loops(app.clone());

    let scheme = if cfg.tls().is_some() { "https" } else { "http" };
    let shown = if cfg.listen.contains(':') {
        format!("[{0}]:{1}", cfg.listen, cfg.port)
    } else {
        format!("{}:{}", cfg.listen, cfg.port)
    };
    println!(
        "herdr-planner {} on {scheme}://{shown}  (board: {}; login: {}; tls: {})",
        app::VERSION,
        cfg.data_dir.display(),
        if cfg.auth_required() { "on" } else { "off" },
        if cfg.tls().is_some() { "on" } else { "off" }
    );

    let router = server::router(app);
    if let Some((cert, key)) = cfg.tls() {
        let tls = match axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await {
            Ok(t) => t,
            Err(e) => {
                eprintln!("herdr-planner: TLS: {e}");
                std::process::exit(1);
            }
        };
        if let Err(e) = axum_server::bind_rustls(addr, tls)
            .serve(router.into_make_service_with_connect_info::<SocketAddr>())
            .await
        {
            eprintln!("herdr-planner: {e}");
            std::process::exit(1);
        }
    } else {
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("herdr-planner: {addr}: {e}");
                std::process::exit(1);
            }
        };
        if let Err(e) = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown_signal())
        .await
        {
            eprintln!("herdr-planner: {e}");
            std::process::exit(1);
        }
    }
}

/// The background work: the board re-read when files change (and once a minute as a safety net), the
/// clones' lag, the search index and the model lists.
fn spawn_loops(app: Shared) {
    let a = app.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = a.poke.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(60)) => {}
            }
            refresh(&a).await;
            memory_api::keep_indexes(&a).await;
            app::index_snapshot(&a).await;
            // Bursts of changes (a working agent rewrites its record all the time) merge into one read.
            tokio::time::sleep(Duration::from_millis(3500)).await;
        }
    });
    let a = app.clone();
    tokio::spawn(async move {
        app::index_chat(&a).await;
        app::index_snapshot(&a).await;
    });
    let a = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        loop {
            app::update_lag(&a).await;
            tokio::time::sleep(Duration::from_secs(300)).await;
        }
    });
    let a = app;
    tokio::spawn(async move {
        loop {
            let programs: Vec<String> = {
                let snap = a.snapshot.read().await;
                let mut set: std::collections::BTreeSet<String> = ["claude", "omp", "pi", "codex"].iter().map(|s| s.to_string()).collect();
                for t in &snap.tasks {
                    for p in [t.app.as_str(), t.worker_agent.split('/').next().unwrap_or("")] {
                        if models::valid_program(p) {
                            set.insert(p.to_string());
                        }
                    }
                }
                set.into_iter().collect()
            };
            models::warm(&a.cfg, &programs).await;
            tokio::time::sleep(Duration::from_secs(600)).await;
        }
    });
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        Ok(mut term) => {
            tokio::select! {
                _ = ctrl_c => {}
                _ = term.recv() => {}
            }
        }
        Err(_) => ctrl_c.await,
    }
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(2)).await;
        std::process::exit(0);
    });
}
