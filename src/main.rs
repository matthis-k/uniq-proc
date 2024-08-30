use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::from_str;
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::{collections::HashMap, fs, process::Command};
use std::{io::Write, path::PathBuf};
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
    #[arg(
        short,
        default_value_t = false,
        help = "Continue from last daemon state"
    )]
    keep: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Adds or overwrites a command
    Add { name: String, command: String },
    /// Removes a command
    Remove { name: String },
    /// Lists all commands
    List,
    /// Executes a command
    Execute { name: String },
    /// Kills a process
    Kill { name: String },
    /// Kills and re-execute a process
    Restart { name: String },
    /// Kills or executes a process, depending on if a process for that name already exists
    Toggle { name: String },
    /// Start a deamon
    Daemon,
}

#[derive(Clone, Serialize, Deserialize)]
enum Message {
    /// Adds or overwrites a command
    Add { name: String, command: String },
    /// Removes a command
    Remove { name: String },
    /// Lists all commands
    List,
    /// Check if daemon is running
    Alive,
    /// Executes a command
    Execute { name: String },
    /// Kills a process
    Kill { name: String },
    /// Kills and re-execute a process
    Restart { name: String },
    /// Kills or executes a process, depending on if a process for that name already exists
    Toggle { name: String },
}

impl TryFrom<Commands> for Message {
    type Error = String;

    fn try_from(value: Commands) -> Result<Self, Self::Error> {
        match value {
            Commands::Daemon { .. } => Err("Daemon is not a message".into()),
            Commands::Add { name, command } => Ok(Message::Add { name, command }),
            Commands::Remove { name } => Ok(Message::Remove { name }),
            Commands::Execute { name } => Ok(Message::Execute { name }),
            Commands::Kill { name } => Ok(Message::Kill { name }),
            Commands::Restart { name } => Ok(Message::Restart { name }),
            Commands::List => Ok(Message::List),
            Commands::Toggle { name } => Ok(Message::Toggle { name }),
        }
    }
}

#[derive(Default)]
struct Daemon {
    data: Arc<Mutex<DaemonState>>,
}

impl Daemon {
    pub fn new(keep: bool) -> Self {
        Self {
            data: Arc::from(Mutex::from(DaemonState::new(keep))),
        }
    }

    fn list(&self) -> String {
        let data = self.data.lock().expect("working mutex");
        serde_json::to_string(&data.commands).expect("can convert to json")
    }
}

impl Daemon {
    pub fn run(&self) {
        if send_message(Message::Alive) == "running" {
            return;
        }
        let _ = std::fs::remove_file(SOCKET_PATH);
        let running = Arc::from(AtomicBool::new(true));
        let running_clone = running.clone();
        const SOCKET_PATH: &str = "/tmp/uniq-proc.sock";
        std::thread::spawn(move || {
            let mut signals = Signals::new(&[SIGINT, SIGTERM]).unwrap();
            for sig in signals.forever() {
                match sig {
                    _ => {
                        running_clone.store(false, Ordering::SeqCst);
                        break;
                    }
                }
            }
        });

        let socket = std::os::unix::net::UnixListener::bind(SOCKET_PATH)
            .expect("successfull creation of socket");
        socket
            .set_nonblocking(true)
            .expect("can set socket to nonblocking");
        std::thread::scope(|s| {
            while running.load(std::sync::atomic::Ordering::SeqCst) {
                let connection = socket.accept();
                match connection {
                    Ok((mut stream, _)) => {
                        let mut msg_raw = String::new();
                        use std::io::Read;
                        let _ =
                            stream.set_read_timeout(Some(std::time::Duration::from_millis(160)));
                        let _ = stream.read_to_string(&mut msg_raw);
                        let msg = from_str(&msg_raw);
                        s.spawn(move || {
                            let response = match msg {
                                Ok(Message::Add { name, command }) => self.add(name, command),
                                Ok(Message::Remove { name }) => self.remove(name),
                                Ok(Message::Kill { name }) => self.kill(name),
                                Ok(Message::Restart { name }) => self.restart(name),
                                Ok(Message::Toggle { name }) => self.toggle(name),
                                Ok(Message::Execute { name }) => self.execute(name),
                                Ok(Message::List) => self.list(),
                                Ok(Message::Alive) => String::from("running"),
                                Err(_) => String::from("Could parse the command"),
                            };
                            stream.write_all(&response.bytes().collect::<Vec<_>>())
                        });
                    }
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(160)),
                }
            }
        });
        self.data.lock().expect("working mutex").save_state();
        std::fs::remove_file(SOCKET_PATH).expect("can remove socket");
    }
}

#[derive(Default, Serialize, Deserialize)]
struct DaemonState {
    commands: HashMap<String, String>,
    procs: HashMap<String, u32>,
}

impl DaemonState {
    fn get_config_path() -> PathBuf {
        let base_dirs = BaseDirectories::with_prefix("uniq-proc").unwrap();
        base_dirs.place_config_file("config.json").unwrap()
    }
    fn get_state_path() -> PathBuf {
        PathBuf::from("/tmp/uniq-proc.state")
    }
    pub fn write_commands_to_config_dir(&self) {
        let config_path = Self::get_config_path();
        let config_content = serde_json::to_string_pretty(&self.commands).expect("can create json");
        std::fs::write(config_path, config_content).expect("can write config file");
    }

    pub fn save_state(&self) {
        let state_path = Self::get_state_path();
        let state_content = serde_json::to_string_pretty(&self).expect("can create json");
        std::fs::write(state_path, state_content).expect("can write config file");
    }

    pub fn new(keep: bool) -> Self {
        let config_path = Self::get_config_path();
        let last_state_path = Self::get_state_path();
        let mut result = Self::default();
        if keep && last_state_path.exists() {
            let state_content = fs::read_to_string(last_state_path).unwrap();
            let parsed = from_str::<Self>(&state_content).expect("valid json");
            if !config_path.exists() {
                result.commands = parsed.commands;
            }
            result.procs = parsed.procs;
        }
        if config_path.exists() {
            let config_content = fs::read_to_string(config_path).unwrap();
            result.commands = from_str(&config_content).expect("valid json");
        }
        result
    }
}

impl Daemon {
    pub fn execute(&self, name: String) -> String {
        let command;
        {
            let data = self.data.lock().expect("working mutex");
            let is_running = data.procs.get(&name).is_some();
            command = if !is_running {
                data.commands.get(&name).cloned()
            } else {
                None
            };
        }
        let Some(command) = command else {
            return format!("{name} is not registered yet");
        };
        let mut process = Command::new("sh").arg("-c").arg(command).spawn().unwrap();
        let pid = process.id();
        {
            let mut data = self.data.lock().expect("no poisioed lock");
            data.procs.insert(name.clone(), pid);
            data.save_state();
        }
        let _ = process.wait();
        {
            let mut data = self.data.lock().expect("working mutex");
            if data.procs.get(&name).filter(|&id| *id == pid).is_some() {
                data.procs.remove(&name);
                data.save_state();
                format!("{name} executed successfully")
            } else {
                format!(
                    "{name} executed successfully, but was restarted with very interesting timing"
                )
            }
        }
    }

    pub fn kill(&self, name: String) -> String {
        let mut data = self.data.lock().expect("working mutex");

        let Some(&pid) = data.procs.get(&name) else {
            return format!("{name} was not running via uniq-proc");
        };
        let mut system = System::new();
        system.refresh_processes();
        let Some(process) = system.process((pid as i32).into()) else {
            return format!("Failed to get the process");
        };
        process.kill();
        data.procs.remove(&name);
        data.save_state();
        format!("Successfully killed name")
    }
    pub fn toggle(&self, name: String) -> String {
        let is_running = {
            let l = self.data.lock().expect("no poisoned lock");
            l.procs.get(&name).is_some()
        };
        if is_running {
            self.kill(name)
        } else {
            self.execute(name)
        }
    }

    pub fn add(&self, name: String, command: String) -> String {
        let mut data = self.data.lock().expect("no poisioed lock");
        data.commands.insert(name.clone(), command);
        data.save_state();
        data.write_commands_to_config_dir();
        format!("Added: {}", data.commands.get(&name).unwrap())
    }

    pub fn remove(&self, name: String) -> String {
        let mut data = self.data.lock().expect("no poisioed lock");
        data.commands.remove(&name);
        data.save_state();
        data.write_commands_to_config_dir();
        format!("Removed {name}")
    }

    pub fn restart(&self, name: String) -> String {
        format!("{}\n{}", self.kill(name.clone()), self.execute(name))
    }
}

fn send_message(msg: Message) -> String {
    match std::os::unix::net::UnixStream::connect("/tmp/uniq-proc.sock") {
        Ok(mut stream) => {
            let message = serde_json::to_string(&msg).expect("can convert to json");
            stream
                .write_all(&message.bytes().collect::<Vec<_>>())
                .expect("can write to stream");
            stream
                .set_write_timeout(std::time::Duration::from_millis(160).into())
                .expect("can set read timeout");

            let mut response = String::new();
            match stream.read_to_string(&mut response) {
                Ok(_) => format!("{response}"),
                Err(e) => match msg {
                    Message::Alive => format!("not running"),
                    _ => format!("An error has occured while getting the response: {e}"),
                },
            }
        }
        Err(e) => format!("{e:?}"),
    }
}

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Daemon => {
            let daemon = Daemon::new(cli.keep);
            daemon.run();
        }
        _ => {
            if send_message(Message::Alive) != "running" {
                let mut cmd = std::process::Command::new(
                    std::env::current_exe().expect("can get own executable"),
                );
                if cli.keep {
                    cmd.arg("-k");
                }
                cmd.arg("daemon");
                cmd.spawn().expect("can start command");

                while !PathBuf::from("/tmp/uniq-proc.sock").exists() {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }

            let response = send_message(
                Message::try_from(cli.command)
                    .expect("can convert all that is not Commands::Daemon"),
            );
            println!("{response}");
        }
    }
}
