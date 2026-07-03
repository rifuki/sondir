//! `sondir mcp` — expose doctor/resolve/watch to AI agents over the Model
//! Context Protocol (stdio transport: newline-delimited JSON-RPC 2.0).
//!
//! No SDK: MCP's stdio transport is one JSON object per line on stdin, one
//! response object per line on stdout. We handle the three methods an agent
//! needs — `initialize`, `tools/list`, `tools/call` — plus `ping`, and stay
//! silent on notifications (requests without an `id`). Every tool reuses the
//! same non-printing entry points the CLI uses, so behavior can't drift.

use std::io::{BufRead, Write};

use anyhow::Result;
use serde_json::{json, Value};

use crate::{resolve, run_doctor, watch};

/// MCP spec revision we implement. We echo the client's requested version when
/// they send one (per spec), falling back to this.
const PROTOCOL_VERSION: &str = "2025-06-18";

pub fn serve() -> Result<i32> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(err) => {
                write_message(&mut out, &parse_error(&err.to_string()))?;
                continue;
            }
        };
        // Notifications (no `id`) get no reply — this covers notifications/initialized.
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        let response = dispatch(method, &params, id);
        write_message(&mut out, &response)?;
    }
    Ok(0)
}

fn dispatch(method: &str, params: &Value, id: Value) -> Value {
    match method {
        "initialize" => success(id, initialize_result(params)),
        "ping" => success(id, json!({})),
        "tools/list" => success(id, json!({ "tools": tool_definitions() })),
        "tools/call" => match call_tool(params) {
            Ok(result) => success(id, result),
            Err(err) => tool_error(id, &format!("{err:#}")),
        },
        // notifications/* never reach here (no id); anything else is unknown.
        other => method_not_found(id, other),
    }
}

fn initialize_result(params: &Value) -> Value {
    let protocol = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(PROTOCOL_VERSION);
    json!({
        "protocolVersion": protocol,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "sondir", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "Solana toolchain pre-flight. Call sondir_doctor before deploying/upgrading an Anchor workspace; sondir_resolve to find a compatible dependency version set; sondir_watch to see if an upstream release/gate unlocked a held-back upgrade."
    })
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "sondir_doctor",
            "description": "Read-only pre-flight for an Anchor workspace: SBPF arch vs cluster/litesvm, SIMD-0431 extend surprises, stranded upgrade buffers, IDL init-vs-upgrade, keypair drift, upgrade authority, balance. Returns findings (severity ok/info/warn/fail) with fixes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace root containing Anchor.toml (default \".\")" },
                    "url": { "type": "string", "description": "RPC URL override; default from Anchor.toml provider.cluster" },
                    "offline": { "type": "boolean", "description": "Skip all RPC calls (local checks only)" }
                }
            }
        },
        {
            "name": "sondir_resolve",
            "description": "Find a mutually-compatible version set for Solana ecosystem deps. Accepts aliases (anchor, litesvm, magicblock, light, pyth, switchboard, metaplex, ...) or raw crate names. Lets cargo's resolver search, applies facts-driven pins on conflict, and reports which Agave interface wave you landed on.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "names": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Ecosystem aliases or raw crate names, e.g. [\"anchor\",\"litesvm\",\"magicblock\"]"
                    }
                },
                "required": ["names"]
            }
        },
        {
            "name": "sondir_watch",
            "description": "Check whether an upstream event unlocked a held-back upgrade: litesvm's Agave-4.1 wave (crates.io), SIMD-0500 v0-v2 deploy ban activation (cluster), anchor's pubkey-4 interface wave. Each trigger reports fired/waiting plus what to do when it fires.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "RPC for the SIMD-0500 gate check; default $SONDIR_RPC else public devnet" }
                }
            }
        }
    ])
}

fn call_tool(params: &Value) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("tools/call missing tool name"))?;
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);

    let payload: Value = match name {
        "sondir_doctor" => {
            let path = args
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or(".")
                .to_string();
            let url = args.get("url").and_then(Value::as_str).map(str::to_owned);
            let offline = args
                .get("offline")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let report = run_doctor(std::path::Path::new(&path), url.as_deref(), offline)?;
            serde_json::to_value(&report)?
        }
        "sondir_resolve" => {
            let names: Vec<String> = args
                .get("names")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            if names.is_empty() {
                anyhow::bail!("sondir_resolve requires a non-empty `names` array");
            }
            serde_json::to_value(resolve::resolve(&names)?)?
        }
        "sondir_watch" => {
            let url = args
                .get("url")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| std::env::var("SONDIR_RPC").ok())
                .unwrap_or_else(|| "https://api.devnet.solana.com".into());
            serde_json::to_value(watch::collect(&url))?
        }
        other => anyhow::bail!("unknown tool: {other}"),
    };

    // MCP wraps tool output in a content array; agents read the text block.
    Ok(json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&payload)? }]
    }))
}

fn write_message(out: &mut impl Write, message: &Value) -> Result<()> {
    // One JSON object per line — the stdio transport framing.
    writeln!(out, "{}", serde_json::to_string(message)?)?;
    out.flush()?;
    Ok(())
}

fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// A tool that failed is reported as a successful call carrying isError:true so
/// the model sees the message (MCP convention), not a protocol-level error.
fn tool_error(id: Value, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": message }],
            "isError": true
        }
    })
}

fn method_not_found(id: Value, method: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": -32601, "message": format!("method not found: {method}") }
    })
}

fn parse_error(detail: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": null,
        "error": { "code": -32700, "message": format!("parse error: {detail}") }
    })
}
