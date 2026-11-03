//! Serving tools of this program's own over the Model Context Protocol, on
//! standard input and output: the first-party servers for a person's mail
//! ([`crate::email`]'s `MailServer`), calendars ([`crate::calendar`]) and
//! documents ([`crate::documents`]), started as `sion mcp mail`, `sion mcp
//! calendar` and `sion mcp documents`.
//!
//! They speak both eras of the protocol, as the ledger's server does
//! ([`crate::mcp`]): a request whose `_meta` carries its version
//! (2026-07-28) is answered as a modern one, `server/discover` included;
//! `initialize` opens the handshake of the earlier versions. They are MCP
//! servers like any other: an agent's operator adds and reviews them
//! (`sion agent mcp add`), their tools pinned by digest, and each tool's
//! annotations say what its calls do, so that the runtime holds them as it
//! holds any server's. They keep no state between calls but their
//! connection to the service.

use std::io::{BufRead, Write};

use serde_json::{json, Value};

use crate::error::CliError;

/// What a server says of itself.
#[derive(Clone, Debug)]
pub struct About {
    /// Its name, as `serverInfo` gives it.
    pub name: &'static str,
    /// What it tells the model of its tools.
    pub instructions: String,
}

/// A first-party server: its tools and what calling them does.
pub trait Serve {
    /// What it says of itself.
    fn about(&self) -> About;
    /// Its tools, as `tools/list` gives them.
    fn tools(&self) -> Vec<Value>;
    /// Calls `tool` with `args`: what it says, or why it could not.
    fn call(&mut self, tool: &str, args: &Value) -> Result<String, String>;
}

/// A tool's definition: its name, what it does, its input's schema, and
/// what its calls do (`readOnlyHint`, `openWorldHint`,
/// `destructiveHint`, `idempotentHint`).
pub fn tool(name: &str, description: &str, schema: Value, annotations: Value) -> Value {
    json!({"name": name, "description": description, "inputSchema": schema, "annotations": annotations})
}

/// The answer to one JSON-RPC message, if it wants one.
pub fn handle(server: &mut dyn Serve, message: &Value) -> Option<Value> {
    let id = message.get("id").cloned();
    let method = message["method"].as_str().unwrap_or("");
    let asked = message["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"].as_str();
    let modern = asked.is_some();
    if let Some(version) = asked.filter(|v| *v != crate::mcp::PROTOCOL_VERSION) {
        let id = id?;
        return Some(json!({"jsonrpc": "2.0", "id": id, "error": {
            "code": -32022,
            "message": "Unsupported protocol version",
            "data": {"supported": [crate::mcp::PROTOCOL_VERSION], "requested": version}
        }}));
    }
    let about = server.about();
    let info = json!({"name": about.name, "version": env!("CARGO_PKG_VERSION")});
    let result = match method {
        "server/discover" => Ok(json!({
            "supportedVersions": [crate::mcp::PROTOCOL_VERSION],
            "capabilities": {"tools": {}},
            "_meta": {"io.modelcontextprotocol/serverInfo": info},
            "instructions": about.instructions
        })),
        "initialize" => {
            let requested = message["params"]["protocolVersion"].as_str().unwrap_or("");
            Ok(json!({
                "protocolVersion": if crate::mcp::LEGACY_VERSIONS.contains(&requested) {
                    requested
                } else {
                    crate::mcp::LEGACY_VERSIONS[0]
                },
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": info,
                "instructions": about.instructions
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": server.tools()})),
        "tools/call" => {
            let name = message["params"]["name"].as_str().unwrap_or("");
            let args = message["params"]["arguments"].clone();
            Ok(match server.call(name, &args) {
                Ok(text) => json!({"content": [{"type": "text", "text": text}], "isError": false}),
                Err(why) => json!({"content": [{"type": "text", "text": why}], "isError": true}),
            })
        }
        _ if method.starts_with("notifications/") => return None,
        _ => Err(json!({"code": -32601, "message": format!("method not found: {method}")})),
    };
    let id = id?;
    Some(match result {
        Ok(mut result) => {
            if modern {
                if let Value::Object(fields) = &mut result {
                    fields.insert("resultType".into(), json!("complete"));
                }
            }
            json!({"jsonrpc": "2.0", "id": id, "result": result})
        }
        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
    })
}

/// Serves `server` on standard input and output until input ends.
pub fn serve(server: &mut dyn Serve) -> Result<(), CliError> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle(server, &message),
            Err(e) => Some(json!({"jsonrpc": "2.0", "id": null,
                "error": {"code": -32700, "message": e.to_string()}})),
        };
        if let Some(response) = response {
            writeln!(stdout, "{response}").map_err(|e| CliError::Usage(e.to_string()))?;
            stdout.flush().map_err(|e| CliError::Usage(e.to_string()))?;
        }
    }
    Ok(())
}

/// A string argument, trimmed; an error naming it if it is missing.
pub fn text(args: &Value, key: &str) -> Result<String, String> {
    args[key]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("give {key}"))
}

/// A list of strings: an array, or one string with commas.
pub fn list(args: &Value, key: &str) -> Vec<String> {
    match &args[key] {
        Value::Array(items) => items
            .iter()
            .filter_map(|i| i.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect(),
        Value::String(s) => s
            .split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    impl Serve for Echo {
        fn about(&self) -> About {
            About {
                name: "echo",
                instructions: "Echoes.".into(),
            }
        }
        fn tools(&self) -> Vec<Value> {
            vec![tool(
                "echo",
                "Say it back.",
                json!({"type": "object"}),
                json!({"readOnlyHint": true}),
            )]
        }
        fn call(&mut self, tool: &str, args: &Value) -> Result<String, String> {
            match tool {
                "echo" => text(args, "text"),
                other => Err(format!("no tool {other}")),
            }
        }
    }

    #[test]
    fn both_eras_are_served() {
        let meta = json!({"io.modelcontextprotocol/protocolVersion": "2026-07-28"});
        let found = handle(&mut Echo, &json!({"jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": {"_meta": meta}})).unwrap();
        assert_eq!(found["result"]["supportedVersions"][0], "2026-07-28");
        assert_eq!(found["result"]["resultType"], "complete");
        let said = handle(
            &mut Echo,
            &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "echo", "arguments": {"text": "hi"}, "_meta": meta}}),
        )
        .unwrap();
        assert_eq!(said["result"]["content"][0]["text"], "hi");
        let hello = handle(
            &mut Echo,
            &json!({"jsonrpc": "2.0", "id": 3, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18"}}),
        )
        .unwrap();
        assert_eq!(hello["result"]["protocolVersion"], "2025-06-18");
        let failed = handle(&mut Echo, &json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "echo", "arguments": {}}})).unwrap();
        assert_eq!(failed["result"]["isError"], true);
        assert!(handle(
            &mut Echo,
            &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
        )
        .is_none());
        let old = handle(
            &mut Echo,
            &json!({"jsonrpc": "2.0", "id": 5, "method": "tools/list",
            "params": {"_meta": {"io.modelcontextprotocol/protocolVersion": "2025-01-01"}}}),
        )
        .unwrap();
        assert_eq!(old["error"]["code"], -32022);
        assert_eq!(
            list(&json!({"to": "a@x.org, b@y.org"}), "to"),
            ["a@x.org", "b@y.org"]
        );
    }
}
