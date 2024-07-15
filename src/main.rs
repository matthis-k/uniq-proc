use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{from_str, to_string_pretty};
use std::{collections::HashMap, fs, process::Command};
use std::{fs::File, io::Write, path::PathBuf};
use sysinfo::{ProcessExt, System, SystemExt};
use xdg::BaseDirectories;

#[derive(Parser)]
#[command(
    name = "uniq-proc",
    version = "1.0",
    about = "Manages unique processes"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Adds or overwrites a command
    Add { name: String, command: String },
    /// Removes a command
    Remove { name: String },
    /// Executes a command
    Execute { name: String },
    /// Kills a process
    Kill { name: String },
    /// Kills and re-execute a process
    Restart { name: String },
    /// Kills or executes a process, depending on if a process for that name already exists
    Toggle { name: String },
}

#[derive(Serialize, Deserialize)]
struct Config {
    commands: HashMap<String, String>,
}

#[derive(Serialize, Deserialize)]
struct ProcessMap {
    processes: HashMap<String, u32>,
}

fn get_config_path() -> PathBuf {
    let base_dirs = BaseDirectories::with_prefix("uniq-proc").unwrap();
    base_dirs.place_config_file("config.json").unwrap()
}

fn get_process_map_path() -> PathBuf {
    PathBuf::from("/tmp/uniq-proc.json")
}

fn read_config() -> Config {
    let config_path = get_config_path();
    if config_path.exists() {
        let config_content = fs::read_to_string(config_path).unwrap();
        from_str(&config_content).unwrap()
    } else {
        Config {
            commands: HashMap::new(),
        }
    }
}

fn write_config(config: &Config) {
    let config_path = get_config_path();
    let config_content = to_string_pretty(config).unwrap();
    let mut file = File::create(config_path).unwrap();
    file.write_all(config_content.as_bytes()).unwrap();
}

fn read_process_map() -> ProcessMap {
    let process_map_path = get_process_map_path();
    if process_map_path.exists() {
        let process_map_content = fs::read_to_string(process_map_path).unwrap();
        from_str(&process_map_content).unwrap()
    } else {
        ProcessMap {
            processes: HashMap::new(),
        }
    }
}

fn write_process_map(process_map: &ProcessMap) {
    let process_map_path = get_process_map_path();
    let process_map_content = to_string_pretty(process_map).unwrap();
    let mut file = File::create(process_map_path).unwrap();
    file.write_all(process_map_content.as_bytes()).unwrap();
}

fn add_command(name: &str, command: &str) {
    let mut config = read_config();
    config
        .commands
        .insert(name.to_string(), command.to_string());
    write_config(&config);
}

fn remove_command(name: &str) {
    let mut config = read_config();
    config.commands.remove(name);
    write_config(&config);
}

fn execute_command(name: &str) {
    let config = read_config();
    if let Some(command) = config.commands.get(name) {
        let mut process = Command::new("sh").arg("-c").arg(command).spawn().unwrap();
        let pid = process.id();

        let mut process_map = read_process_map();
        process_map.processes.insert(name.to_string(), pid);
        write_process_map(&process_map);

        let _ = process.wait();

        let mut process_map = read_process_map();
        if let Some(&terminated_pid) = process_map.processes.get(name) {
            if terminated_pid == pid {
                process_map.processes.remove(&name.to_string());
                write_process_map(&process_map);
            }
        }
    } else {
        eprintln!("Command not found for name: {}", name);
    }
}

fn kill_process(name: &str) {
    let mut process_map = read_process_map();
    if let Some(&pid) = process_map.processes.get(name) {
        let mut system = System::new();
        system.refresh_processes();
        if let Some(process) = system.process((pid as i32).into()) {
            process.kill();
            process_map.processes.remove(name);
            write_process_map(&process_map);
        } else {
            eprintln!("Process not found for pid: {}", pid);
        }
    } else {
        eprintln!("No process running for name: {}", name);
    }
}

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Add { name, command } => add_command(name, command),
        Commands::Remove { name } => remove_command(name),
        Commands::Execute { name } => execute_command(name),
        Commands::Kill { name } => kill_process(name),
        Commands::Restart { name } => {
            kill_process(name);
            execute_command(name);
        }
        Commands::Toggle { name } => {
            let process_map = read_process_map();
            if process_map.processes.get(&name.to_string()).is_some() {
                kill_process(name);
            } else {
                execute_command(name);
            }
        }
    }
}
