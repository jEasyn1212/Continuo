#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use continuo_core::api::{envelope, Policy, Service};
use serde_json::Value;
use std::path::PathBuf;
struct AppState {
    path: PathBuf,
    policy: Policy,
}
use tauri::Manager;

#[tauri::command]
async fn api_call(app: tauri::AppHandle, method: String, params: Value) -> Value {
    match tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let mut policy = state.policy;
        // Native UI grants only this confirmed request, not a persistent worker permission.
        if method == "system.describe"
            || (method == "mcp.probe" && params["confirm_execution"] == true)
        {
            policy.probes = true;
        }
        envelope(
            Service::open(&state.path, policy).and_then(|service| service.call(&method, params)),
        )
    })
    .await
    {
        Ok(result) => result,
        Err(_) => envelope(Err(continuo_core::Error::new(
            "service_unavailable",
            "Application operation failed",
        ))),
    }
}
fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let path = continuo_core::default_data_dir()?;
            let policy = Policy {
                writes: true,
                sync: true,
                admin: true,
                probes: false,
            };
            Service::open(&path, policy)?;
            app.manage(AppState { path, policy });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![api_call])
        .run(tauri::generate_context!())
        .expect("Continuo desktop failed to start");
}
