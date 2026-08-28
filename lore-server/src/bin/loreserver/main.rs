// SPDX-FileCopyrightText: 2026 Epic Games, Inc.
// SPDX-License-Identifier: MIT
// Copyright Epic Games, Inc. All Rights Reserved.

use std::env;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::thread::sleep;
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
    rebac_healthcheck(timeout)?;
    Ok(())
}

/// A host-network Lore task is not ready until its colocated Auth Gateway has
/// accepted the private ReBAC socket. This is intentionally a TCP probe: the
/// authenticated ReBAC RPC itself is exercised by the service integration tests.
fn rebac_healthcheck(timeout: Duration) -> Result<()> {
    let rebac_url =
        env::var("LORE_REBAC_URL").context("LORE_REBAC_URL is required for readiness")?;
    let authority = rebac_url
        .strip_prefix("http://")
        .or_else(|| rebac_url.strip_prefix("https://"))
        .unwrap_or(&rebac_url)
        .split('/')
        .next()
        .unwrap_or_default();
    let address = if authority.contains(':') {
        authority.to_owned()
    } else {
        format!("{authority}:8087")
    };
    let socket = address
        .to_socket_addrs()
        .with_context(|| format!("resolve ReBAC endpoint at {address}"))?
        .next()
        .context("ReBAC endpoint resolved to no addresses")?;
    // Keep readiness below its ECS five-second command timeout while still
    // tolerating Auth starting after Lore. The application client has its own
    // longer retry policy for request-time connections.
    let max_attempts = env::var("LORE_REBAC_HEALTH_MAX_ATTEMPTS")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(4);
    let connect_timeout = timeout.min(Duration::from_millis(500));
    let mut last_error = None;
    for attempt in 1..=max_attempts {
        match TcpStream::connect_timeout(&socket, connect_timeout) {
            Ok(_) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                if attempt < max_attempts {
                    let backoff_ms = 100_u64.saturating_mul(1_u64 << (attempt - 1).min(3));
                    sleep(Duration::from_millis(backoff_ms));
                }
            }
        }
    }
    bail!(
        "ReBAC endpoint at {address} remained unavailable after {max_attempts} readiness attempts: {}",
        last_error.expect("a failed retry loop has an error"),
    )
}
