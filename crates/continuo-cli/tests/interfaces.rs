use serde_json::{json, Value};
use std::{
    io::Write,
    process::{Command, Stdio},
};

fn rpc(writes: bool, extra: &[Value]) -> Vec<Value> {
    let dir = tempfile::tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_continuo"));
    cmd.arg("--data-dir").arg(dir.path()).arg("mcp");
    if writes {
        cmd.arg("--allow-writes");
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut requests = vec![
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}),
    ];
    requests.extend_from_slice(extra);
    {
        let mut input = child.stdin.take().unwrap();
        for r in requests {
            writeln!(input, "{r}").unwrap();
        }
    }
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success());
    String::from_utf8(result.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
#[test]
fn mcp_handshake_read_only_defaults_and_structured_errors() {
    let out = rpc(
        false,
        &[
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"continuo_entity_create","arguments":{"kind":"task","name":"x","data":{}}}}),
        ],
    );
    assert_eq!(out[0]["error"]["code"], -32002);
    assert_eq!(out[1]["result"]["protocolVersion"], "2025-11-25");
    assert!(!out[2]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["name"] == "continuo_entity_create"));
    assert_eq!(out[3]["error"]["code"], -32602);
    let write = rpc(
        true,
        &[
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"continuo_entity_create","arguments":{"kind":"task","name":"from MCP","data":{}}}}),
            json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"continuo_entity_update","arguments":{"id":"missing","expected_revision":"stale"}}}),
        ],
    );
    assert_eq!(write[3]["result"]["structuredContent"]["ok"], true);
    assert_eq!(write[4]["result"]["isError"], true);
    assert_eq!(
        write[4]["result"]["structuredContent"]["error"]["code"],
        "not_found"
    );
    assert!(!write[2]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["name"] == "continuo_sync_configure" || t["name"] == "continuo_sync_run"));
}
#[test]
fn cli_json_input_and_database_persist_between_invocations() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input.json");
    std::fs::write(
        &input,
        json!({"kind":"task","name":"literal `$(text)`","data":{"status":"active"}}).to_string(),
    )
    .unwrap();
    let created = Command::new(env!("CARGO_BIN_EXE_continuo"))
        .arg("--data-dir")
        .arg(dir.path().join("state"))
        .args(["call", "entity.create", "--input"])
        .arg(&input)
        .output()
        .unwrap();
    assert!(created.status.success());
    let value: Value = serde_json::from_slice(&created.stdout).unwrap();
    assert_eq!(value["data"]["heads"][0]["name"], "literal `$(text)`");
    let status = Command::new(env!("CARGO_BIN_EXE_continuo"))
        .arg("--data-dir")
        .arg(dir.path().join("state"))
        .arg("status")
        .output()
        .unwrap();
    assert!(status.status.success());
    let value: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(value["data"]["counts"]["task"], 1);
}
