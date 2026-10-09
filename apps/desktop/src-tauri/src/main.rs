#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use continuo_core::api::{envelope, Policy, Service};
use serde_json::Value;
use std::sync::Mutex;
use tauri::Manager;

#[tauri::command]
async fn api_call(app: tauri::AppHandle, method: String, params: Value) -> Value {
    match tauri::async_runtime::spawn_blocking(move || {
        let service = app.state::<Mutex<Service>>();
        let response = match service.lock() {
            Ok(service) => envelope(service.call(&method, params)),
            Err(_) => envelope(Err(continuo_core::Error::new(
                "service_unavailable",
                "Application service lock is unavailable",
            ))),
        };
        response
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
            app.manage(Mutex::new(Service::open(
                &path,
                Policy {
                    writes: true,
                    sync: true,
                    admin: true,
                },
            )?));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![api_call])
        .run(tauri::generate_context!())
        .expect("Continuo desktop failed to start");
}
