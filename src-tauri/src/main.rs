#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.iter().any(|arg| arg == "--mcp-stdio") {
        if args.len() != 1 {
            eprintln!("MCP stdio mode accepts no additional command-line arguments");
            std::process::exit(2);
        }
        // Proxy mode must branch before desktop setup/single-instance handling:
        // the existing desktop process remains the sole database/filesystem owner.
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let endpoint = std::env::var("AMM_MCP_ENDPOINT")?;
            let token = std::env::var("AMM_MCP_TOKEN")?;
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?
                .block_on(agent_mod_manager_lib::mcp_server::run_stdio_proxy(
                    &endpoint, &token,
                ))?;
            Ok(())
        })();
        if result.is_err() {
            // Never echo environment values, endpoint credentials or SDK errors.
            eprintln!("MCP proxy failed; verify the desktop service and credential environment");
            std::process::exit(1);
        }
        return;
    }
    agent_mod_manager_lib::run();
}
