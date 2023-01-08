extern crate core;

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use axum::Router;
use axum::routing::get;
use env_logger::Env;
use log::{error, info};
use once_cell::sync::OnceCell;
use tokio::select;
use tokio::task::JoinSet;

use crate::config::{Config, read_config};
use crate::endpoints::find_mail;
use crate::mail::{init_mail, tail_mail, tail_mail_log};
use crate::tail::FileTail;

mod endpoints;
mod mail;
mod tail;
mod config;

pub(crate) static CONFIG: OnceCell<Config> = OnceCell::new();

/// Start the API to query for mails and subjects
async fn start_http() -> Result<String> {
    let socket_addr: SocketAddr = format!("{}:{}", Config::global().listen.ip, Config::global().listen.port, ).parse()?;
    info!("Server listening on {}", socket_addr);
    let app = Router::new()
        .route("/find_mail", get(find_mail));
    axum::Server::bind(&socket_addr)
        .serve(app.into_make_service()).await?;
    Ok(String::from("HTTP server stopped"))
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init_from_env(Env::default().filter_or("RUST_LOG", "info"));
    CONFIG.set(read_config()?).unwrap();
    let mut tasks = JoinSet::new();
    tasks.spawn(start_http());
    tasks.spawn(init_mail());
    tasks.spawn(tail_mail_log());
    tasks.spawn(tail_mail(Duration::from_secs(Config::global().mail_parsing_delay)));
    loop {
        select! {
            _ = tokio::signal::ctrl_c() => {
                    info!("CTRL + C received. Shutting down all tasks.");
                    tasks.shutdown().await;
                    return Ok(())
                },
            res = tasks.join_next() => {
                let res = res.unwrap()?;
                match res {
                    Ok(val) => info!("{val}"),
                    Err(why) => {
                        error!("{why}");
                        return Err(why);
                    }
                }
                if tasks.is_empty() {
                    info!("all tasks finished");
                    return Ok(())
                }
            }
        }
    }
}
