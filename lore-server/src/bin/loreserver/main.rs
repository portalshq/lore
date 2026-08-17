// SPDX-FileCopyrightText: 2026 Epic Games, Inc.
// SPDX-License-Identifier: MIT
// Copyright Epic Games, Inc. All Rights Reserved.

use std::env;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use lore_server::server::server_main;
use lore_server::server_config::ServerConfig;

fn main() -> Result<()> {
    if env::args().nth(1).as_deref() == Some("healthcheck") {
        return healthcheck();
    }
    server_main(ServerConfig::default())
}

/// Shell-free container health probe for the distroless runtime image.
fn healthcheck() -> Result<()> {
    let address =
        env::var("LORE_HEALTHCHECK_ADDR").unwrap_or_else(|_| "127.0.0.1:41339".to_owned());
    let timeout = Duration::from_secs(4);
    let mut stream = TcpStream::connect(&address)
        .with_context(|| format!("connect to Lore health endpoint at {address}"))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream
        .write_all(b"GET /health_check HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let status = response.lines().next().unwrap_or_default();
    if !matches!(status, "HTTP/1.1 200 OK" | "HTTP/1.0 200 OK") {
        bail!("Lore health endpoint returned {status:?}");
    }
    Ok(())
}
