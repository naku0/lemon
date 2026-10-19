use app::cli::{self, Action, Cli};
use core_types::Result;
use std::time::Duration;

pub fn run(cli: &Cli) -> Result<()> {
    let mut platform = plugins::Platform::open_default().map_err(|e| e.to_string())?;
    match cli.command.as_ref() {
        Some(Action::Plugins { command }) => manage_registry(&mut platform, command),
        Some(Action::Plugin { command }) => run_plugin(&mut platform, command, cli),
        _ => Err("plugin command dispatch error".into()),
    }
}

fn manage_registry(platform: &mut plugins::Platform, command: &cli::PluginsAction) -> Result<()> {
    match command {
        cli::PluginsAction::Add { plugin_dir } => {
            let info = platform.add(plugin_dir).map_err(|e| e.to_string())?;
            print_plugin(&info);
        }
        cli::PluginsAction::List => {
            let list = platform.list();
            if list.is_empty() {
                println!("No plugins registered");
            }
            for info in list {
                print_plugin(&info);
            }
        }
        cli::PluginsAction::Inspect { plugin_id_or_dir } => {
            let info = platform
                .inspect(plugin_id_or_dir)
                .map_err(|e| e.to_string())?;
            print_plugin(&info);
            if let Some(manifest) = info.manifest {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?
                );
            }
        }
        cli::PluginsAction::Remove { plugin_id } => {
            platform.remove(plugin_id).map_err(|e| e.to_string())?;
            println!("Removed {plugin_id} from the registry; plugin files were not deleted");
        }
        cli::PluginsAction::Enable { plugin_id } => print_plugin(
            &platform
                .set_enabled(plugin_id, true)
                .map_err(|e| e.to_string())?,
        ),
        cli::PluginsAction::Disable { plugin_id } => print_plugin(
            &platform
                .set_enabled(plugin_id, false)
                .map_err(|e| e.to_string())?,
        ),
    }
    Ok(())
}

fn run_plugin(
    platform: &mut plugins::Platform,
    command: &cli::PluginAction,
    cli: &Cli,
) -> Result<()> {
    let cli::PluginAction::Run {
        plugin_id_or_dir,
        input,
        output,
        overwrite,
        timeout,
    } = command;
    let config = cli
        .config
        .clone()
        .ok_or("plugin run: --config CONFIG_JSON is required")?;
    let report = platform
        .run(&plugins::RunOptions {
            plugin: plugin_id_or_dir.clone(),
            input: input.clone(),
            config,
            output: output.clone(),
            overwrite: *overwrite,
            timeout: Duration::from_secs(*timeout),
        })
        .map_err(|e| e.to_string())?;
    println!(
        "Plugin {} {}: {}",
        report.manifest.id, report.manifest.version, report.result.status
    );
    for warning in &report.result.warnings {
        println!("Warning: {warning}");
    }
    for artifact in &report.result.artifacts {
        println!(
            "{}: {} — {}",
            artifact.kind,
            report.output.join(&artifact.path).display(),
            artifact.description
        );
    }
    Ok(())
}

fn print_plugin(info: &plugins::PluginInfo) {
    let (name, version, api) =
        info.manifest
            .as_ref()
            .map_or((info.id.as_str(), "?", "?".into()), |manifest| {
                (
                    manifest.display_name.as_str(),
                    manifest.version.as_str(),
                    manifest.api_version.to_string(),
                )
            });
    println!(
        "{} ({}) v{} | API {} | {}\n  {}{}",
        name,
        info.id,
        version,
        api,
        info.state,
        info.path.display(),
        info.reason
            .as_ref()
            .map(|reason| format!("\n  {reason}"))
            .unwrap_or_default()
    );
}
