//! The Tauri shell of the Siontology app, for desktop and mobile alike:
//! every command forwards to `sion_app_core::AppCore`, which holds the
//! device's key and verifies what it shows.

use serde_json::Value;
use sion_app_core::AppCore;
use tauri::{Manager, State};

type Answer = Result<Value, String>;

fn flat(result: Result<Value, sion_app_core::AppError>) -> Answer {
    result.map_err(|e| e.to_string())
}

// Every command runs on Tauri's thread pool (`async`), and the core takes
// `&self` and holds no lock while it talks to the node: a verification in
// progress does not hold up a balance.

#[tauri::command(async)]
fn me(core: State<'_, AppCore>) -> Answer {
    Ok(core.me())
}

#[tauri::command(async)]
fn settings(core: State<'_, AppCore>) -> Answer {
    Ok(core.settings())
}

#[tauri::command(async)]
fn set_node(core: State<'_, AppCore>, node: String) -> Answer {
    flat(core.set_node(&node))
}

#[tauri::command(async)]
fn pin_chain(core: State<'_, AppCore>, chain: Option<String>) -> Answer {
    flat(core.pin_chain(chain.as_deref()))
}

#[tauri::command(async)]
fn status(core: State<'_, AppCore>) -> Answer {
    flat(core.status())
}

#[tauri::command(async)]
fn balance(core: State<'_, AppCore>) -> Answer {
    flat(core.balance())
}

#[tauri::command(async)]
fn faucet(core: State<'_, AppCore>, amount: String) -> Answer {
    flat(core.faucet(&amount))
}

#[tauri::command(async)]
fn send(core: State<'_, AppCore>, to: String, amount: String) -> Answer {
    flat(core.send(&to, &amount))
}

#[tauri::command(async)]
fn verify(core: State<'_, AppCore>) -> Answer {
    flat(core.verify_latest())
}

#[tauri::command(async)]
fn history(core: State<'_, AppCore>) -> Answer {
    Ok(core.history())
}

#[tauri::command(async)]
fn pools(core: State<'_, AppCore>) -> Answer {
    flat(core.pools())
}

#[tauri::command(async)]
fn delegate(core: State<'_, AppCore>, validator: String, amount: String) -> Answer {
    flat(core.delegate(&validator, &amount))
}

#[tauri::command(async)]
fn delegations(core: State<'_, AppCore>) -> Answer {
    flat(core.delegations())
}

#[tauri::command(async)]
fn delegation_action(core: State<'_, AppCore>, action: String, delegation: String) -> Answer {
    flat(core.delegation_action(&action, &delegation))
}

#[tauri::command(async)]
fn set_storage(core: State<'_, AppCore>, enabled: bool, max_mib: u64) -> Answer {
    flat(core.set_storage(enabled, max_mib))
}

#[tauri::command(async)]
fn storage_tick(core: State<'_, AppCore>) -> Answer {
    flat(core.storage_tick())
}

#[tauri::command(async)]
fn storage_status(core: State<'_, AppCore>) -> Answer {
    flat(core.storage_status())
}

#[tauri::command(async)]
fn agents(core: State<'_, AppCore>) -> Answer {
    flat(core.agents())
}

#[tauri::command(async)]
fn watch_agent(core: State<'_, AppCore>, agent: String) -> Answer {
    flat(core.watch_agent(&agent))
}

#[tauri::command(async)]
fn unwatch_agent(core: State<'_, AppCore>, agent: String) -> Answer {
    flat(core.unwatch_agent(&agent))
}

#[tauri::command(async)]
fn find_agents(core: State<'_, AppCore>) -> Answer {
    flat(core.find_agents())
}

#[tauri::command(async)]
fn agent_action(core: State<'_, AppCore>, action: String, agent: String) -> Answer {
    flat(core.agent_action(&action, &agent))
}

/// Starts the app.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = std::env::var_os("SION_APP_DIR")
                .map(std::path::PathBuf::from)
                .map(Ok)
                .unwrap_or_else(|| app.path().app_data_dir())?;
            let core = AppCore::open(&dir)?;
            if let Ok(node) = std::env::var("SION_NODE") {
                core.set_node(&node)?;
            }
            app.manage(core);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            me,
            settings,
            set_node,
            pin_chain,
            status,
            balance,
            faucet,
            send,
            verify,
            history,
            pools,
            delegate,
            delegations,
            delegation_action,
            set_storage,
            storage_tick,
            storage_status,
            agents,
            watch_agent,
            unwatch_agent,
            find_agents,
            agent_action
        ])
        .run(tauri::generate_context!())
        .expect("the Siontology app failed to start");
}
