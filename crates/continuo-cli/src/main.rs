mod mcp;

use continuo_core::{
    api::{envelope, operations, Policy, Service},
    Error, Result,
};
use serde_json::{json, Value};
use std::{io::Read, path::PathBuf};

fn main() {
    if let Some(result) =
        continuo_core::process::internal_entry(&std::env::args().skip(1).collect::<Vec<_>>())
    {
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    let result = run();
    if let Err(error) = result {
        if std::env::args().any(|a| a == "mcp") {
            eprintln!("{}", envelope(Err(error)));
        } else {
            println!("{}", envelope(Err(error)));
        }
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Continuo {}\n\ncontinuo [--data-dir PATH] status|agents|describe\ncontinuo [--data-dir PATH] call METHOD [--input FILE|-] [--allow-sync] [--allow-mcp-probes] [--allow-managed-processes]\ncontinuo [--data-dir PATH] mcp [--allow-writes] [--allow-sync] [--allow-admin] [--allow-mcp-probes] [--allow-managed-processes]\n\nInput is a JSON object; omitted input defaults to {{}}. All API responses are JSON.\nMCP defaults to read-only; --allow-sync and --allow-admin require --allow-writes.\nSecrets and existing agent configurations are not imported.", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let data_dir = take_value(&mut args, "--data-dir")?
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CONTINUO_DATA_DIR").map(PathBuf::from));
    let writes = take_flag(&mut args, "--allow-writes");
    let sync = take_flag(&mut args, "--allow-sync");
    let admin = take_flag(&mut args, "--allow-admin");
    let probes = take_flag(&mut args, "--allow-mcp-probes");
    let processes = take_flag(&mut args, "--allow-managed-processes");
    let input = take_value(&mut args, "--input")?;
    let command = args.first().map(String::as_str).unwrap_or("");
    if command == "describe" {
        if args.len() != 1 || input.is_some() {
            return Err(Error::new(
                "usage",
                "describe accepts no positional arguments or input",
            ));
        }
        println!("{}", envelope(Ok(json!({"operations":operations()}))));
        return Ok(());
    }
    if !matches!(command, "status" | "agents" | "call" | "mcp") {
        return Err(Error::new("usage", "Unknown command; use --help"));
    }
    if (command == "call" && args.len() != 2) || (command != "call" && args.len() != 1) {
        return Err(Error::new("usage", "Unexpected command arguments"));
    }
    if command == "mcp" && input.is_some() {
        return Err(Error::new(
            "usage",
            "MCP owns stdin; --input is not supported",
        ));
    }
    if command == "mcp" && sync && !writes {
        return Err(Error::new(
            "usage",
            "MCP synchronization requires --allow-writes and --allow-sync",
        ));
    }
    if command == "mcp" && admin && !writes {
        return Err(Error::new(
            "usage",
            "MCP administration requires --allow-writes and --allow-admin",
        ));
    }
    if probes && command == "mcp" && (!writes || !admin) {
        return Err(Error::new(
            "usage",
            "--allow-mcp-probes requires --allow-writes and --allow-admin",
        ));
    }
    if processes && command == "mcp" && (!writes || !admin) {
        return Err(Error::new(
            "usage",
            "--allow-managed-processes requires --allow-writes and --allow-admin",
        ));
    }
    let path = match data_dir {
        Some(path) => path,
        None => continuo_core::default_data_dir()?,
    };
    let policy = if command == "mcp" {
        Policy {
            writes,
            sync,
            admin,
            probes,
            processes,
        }
    } else {
        Policy {
            sync,
            probes,
            processes,
            ..Policy::local_user()
        }
    };
    let service = Service::open(&path, policy)?.with_simulator(std::env::current_exe()?);
    if command == "mcp" {
        return mcp::serve(&service);
    }
    let params = match input {
        Some(path) => {
            let mut data = String::new();
            if path == "-" {
                std::io::stdin()
                    .take(1024 * 1024 + 1)
                    .read_to_string(&mut data)?;
            } else {
                std::fs::File::open(path)?
                    .take(1024 * 1024 + 1)
                    .read_to_string(&mut data)?;
            }
            if data.len() > 1024 * 1024 {
                return Err(Error::new("input_too_large", "Input exceeds 1 MiB"));
            }
            serde_json::from_str::<Value>(&data)?
        }
        None => json!({}),
    };
    let method = match command {
        "status" => "system.status",
        "agents" => "agent.list",
        _ => &args[1],
    };
    let result = service.call(method, params);
    let failed = result.is_err();
    println!("{}", envelope(result));
    if failed {
        std::process::exit(1);
    }
    Ok(())
}
fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    if let Some(index) = args.iter().position(|a| a == flag) {
        args.remove(index);
        true
    } else {
        false
    }
}
fn take_value(args: &mut Vec<String>, flag: &str) -> Result<Option<String>> {
    if let Some(index) = args.iter().position(|a| a == flag) {
        args.remove(index);
        if index >= args.len() || args[index].starts_with("--") {
            return Err(Error::new("usage", format!("{flag} needs a value")));
        }
        Ok(Some(args.remove(index)))
    } else {
        Ok(None)
    }
}
