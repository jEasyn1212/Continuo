use continuo_core::{
    api::{envelope, Service},
    Result,
};
use serde_json::{json, Value};
use std::io::{BufRead, Write};

const VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];
const LIMIT: usize = 1024 * 1024;

pub fn serve(service: &Service) -> Result<()> {
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let mut initialized = false;
    let mut ready = false;
    loop {
        // Bounded reader: discard an oversized frame without allocating its full size.
        let mut line = Vec::new();
        let mut oversized = false;
        let mut eof = false;
        loop {
            let available = input.fill_buf()?;
            if available.is_empty() {
                eof = true;
                break;
            }
            let newline = available.iter().position(|b| *b == b'\n');
            let consumed = newline.map(|i| i + 1).unwrap_or(available.len());
            if line.len() + consumed > LIMIT {
                oversized = true;
                line.clear();
            }
            if !oversized {
                line.extend_from_slice(&available[..consumed]);
            }
            input.consume(consumed);
            if newline.is_some() {
                break;
            }
        }
        if eof && line.is_empty() && !oversized {
            break;
        }
        let response = if oversized {
            Some(error(Value::Null, -32700, "Frame exceeds 1 MiB"))
        } else {
            match serde_json::from_slice::<Value>(&line) {
                Err(_) => Some(error(Value::Null, -32700, "Invalid JSON")),
                Ok(request) => handle(service, request, &mut initialized, &mut ready),
            }
        };
        if let Some(response) = response {
            writeln!(output, "{response}")?;
            output.flush()?;
        }
        if eof {
            break;
        }
    }
    Ok(())
}
fn handle(service: &Service, r: Value, initialized: &mut bool, ready: &mut bool) -> Option<Value> {
    let id = r.get("id").cloned();
    let method = r.get("method").and_then(Value::as_str);
    if !r.is_object()
        || r.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || method.is_none()
        || id
            .as_ref()
            .is_some_and(|id| !id.is_string() && !id.is_number())
    {
        return Some(error(Value::Null, -32600, "Invalid JSON-RPC request"));
    }
    let method = method.unwrap();
    if id.is_none() {
        if method == "notifications/initialized" && *initialized {
            *ready = true;
        }
        return None;
    }
    let id = id.unwrap();
    let params = r.get("params").cloned().unwrap_or_else(|| json!({}));
    if !params.is_object() {
        return Some(error(id, -32602, "params must be an object"));
    }
    match method {
        "initialize" => {
            if *initialized {
                return Some(error(id, -32600, "Already initialized"));
            }
            let Some(requested) = params.get("protocolVersion").and_then(Value::as_str) else {
                return Some(error(id, -32602, "protocolVersion is required"));
            };
            if !params.get("capabilities").is_some_and(Value::is_object)
                || !params.get("clientInfo").is_some_and(Value::is_object)
            {
                return Some(error(
                    id,
                    -32602,
                    "capabilities and clientInfo are required",
                ));
            }
            let version = if VERSIONS.contains(&requested) {
                requested
            } else {
                VERSIONS[0]
            };
            *initialized = true;
            Some(success(
                id,
                json!({"protocolVersion":version,"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"continuo","version":env!("CARGO_PKG_VERSION")},"instructions":"Continuo manages explicit local records. Identity is not account or permission isolation. Fetch revisions before mutation. Native launch plans do not execute processes. Never put credentials into entity data."}),
            ))
        }
        "ping" => Some(success(id, json!({}))),
        _ if !*ready => Some(error(
            id,
            -32002,
            "Complete initialize and notifications/initialized first",
        )),
        "tools/list" => {
            if params.get("cursor").is_some() {
                return Some(error(id, -32602, "This catalog is not paginated"));
            }
            let tools: Vec<_>=service.available().into_iter().map(|o|json!({"name":o.tool,"description":o.description,"inputSchema":o.input_schema,"annotations":{"readOnlyHint":!o.writes,"destructiveHint":o.method=="entity.delete"||o.method=="entity.update"||o.method=="entity.resolve","idempotentHint":!o.writes,"openWorldHint":o.network}})).collect();
            Some(success(id, json!({"tools":tools})))
        }
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(error(id, -32602, "Tool name is required"));
            };
            let Some(op) = service.available().into_iter().find(|o| o.tool == name) else {
                return Some(error(id, -32602, "Tool is unknown or not enabled"));
            };
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let result = service.call(op.method, arguments);
            let is_error = result.is_err();
            let data = envelope(result);
            Some(success(
                id,
                json!({"content":[{"type":"text","text":data.to_string()}],"structuredContent":data,"isError":is_error}),
            ))
        }
        _ => Some(error(id, -32601, "Method not found")),
    }
}
fn success(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}
fn error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
