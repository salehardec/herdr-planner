//! Herdr Planner: a local web dashboard for herdr-projects, in the herdr-gpui
//! theme. One binary: the server, the data sources and the embedded page.

mod actions;
mod agent;
mod api;
mod app;
mod attention;
mod cards;
mod config;
mod coordask;
mod fresh;
mod import_tern;
mod jsonscan;
mod model;
mod mrstate;
mod newproject;
mod poller;
mod sources;
mod store;
mod suggest;
mod theme;

use std::net::{Ipv4Addr, SocketAddr};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "herdr-planner [--port N] [--root DIR]\n\nПланировщик проектов herdr-projects на http://127.0.0.1:PORT (по умолчанию {}).\nНастройки: ~/.config/herdr-planner/config.toml (port, root); состояние: ~/.local/state/herdr-planner/state.json.",
            config::DEFAULT_PORT
        );
        return;
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("herdr-planner {}", app::VERSION);
        return;
    }
    let cfg = config::AppConfig::load(&args);

    // Saved state, or the Tern plugin's on first run.
    let state = match store::State::load(&cfg.state_file) {
        Some(s) => s,
        None => {
            let imported = import_tern::import(&import_tern::default_path());
            let s = imported.unwrap_or_default().normalize();
            if let Err(e) = s.save(&cfg.state_file) {
                eprintln!(
                    "herdr-planner: не сохранилось состояние {}: {}",
                    cfg.state_file.display(),
                    e
                );
            }
            if let Some(from) = &s.imported_from {
                eprintln!("herdr-planner: состояние плагина Tern перенесено из {from}");
            }
            s
        }
    };

    let theme = theme::load();
    if let Some(e) = &theme.error {
        eprintln!("herdr-planner: тема herdr-gpui не прочитана ({e}), показана запасная");
    }
    let shared = app::App::new(cfg.clone(), state, theme);

    tokio::spawn(poller::refresh_loop(shared.clone()));
    tokio::spawn(poller::workspace_loop(shared.clone()));
    tokio::spawn(poller::theme_loop(shared.clone()));

    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, cfg.port));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "herdr-planner: порт {} занят или недоступен: {}",
                cfg.port, e
            );
            std::process::exit(1);
        }
    };
    println!(
        "herdr-planner {} на http://{}  (проекты: {}; настройки: {})",
        app::VERSION,
        addr,
        cfg.root.display(),
        cfg.config_file.display()
    );
    let router = api::router(shared);
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server");
}

/// Ctrl-C, or SIGTERM from `systemctl stop`: finish the open requests, then exit.
/// Open event streams (the page's SSE) never end by themselves, so after a short
/// grace the process leaves anyway.
async fn shutdown_signal() {
    wait_for_signal().await;
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        std::process::exit(0);
    });
}

async fn wait_for_signal() {
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
}
