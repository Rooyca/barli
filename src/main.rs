use signal_hook::{consts::signal::*, iterator::Signals};
use std::{
    env,
    ffi::CString,
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Sender, channel},
    },
    thread,
    time::Duration,
};
use wait_timeout::ChildExt;
use x11::xlib;

#[derive(Debug, Clone)]
struct Task {
    prefix: String,
    cmd: String,
    suffix: String,
    interval: u64,
    shell: bool,
    timeout: Option<u64>,
}

#[derive(Debug)]
enum BarliError {
    ConfigRead(std::io::Error),
    ConfigWrite(std::io::Error),
    DisplayOpen,
    InvalidUtf8(std::string::FromUtf8Error),
    SignalHandling(String),
}

impl std::fmt::Display for BarliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BarliError::ConfigRead(e) => write!(f, "[-] Failed to read config: {}", e),
            BarliError::ConfigWrite(e) => write!(f, "[-] Failed to write config: {}", e),
            BarliError::DisplayOpen => write!(f, "[-] Failed to open X11 display"),
            BarliError::InvalidUtf8(e) => write!(f, "[-] Invalid UTF-8 in command output: {}", e),
            BarliError::SignalHandling(e) => write!(f, "[-] Signal handling error: {}", e),
        }
    }
}

impl std::error::Error for BarliError {}

#[derive(Debug, Clone)]
enum AppMessage {
    TaskResult(usize, String),
    ReloadConfig,
    Shutdown,
}

impl Task {
    /// Validates that the task has a command to run
    fn is_valid(&self) -> bool {
        !self.cmd.trim().is_empty()
    }
}

/// Resolves the configuration file path, creating a default if none exists
fn resolve_config_path() -> Result<PathBuf, BarliError> {
    let home = env::var("HOME").unwrap_or_else(|_| ".".into());
    let config_dir = PathBuf::from(&home).join(".config/barli.conf");
    let legacy = PathBuf::from(&home).join(".barli.conf");

    if config_dir.exists() {
        Ok(config_dir)
    } else if legacy.exists() {
        Ok(legacy)
    } else {
        // Create parent directory if it doesn't exist
        if let Some(parent) = config_dir.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(BarliError::ConfigWrite)?;
            }
        }

        // Create a simple default config
        let default = "TIME: :: date :: :: 2 :: \n";
        fs::write(&config_dir, default).map_err(BarliError::ConfigWrite)?;
        Ok(config_dir)
    }
}

/// Parses a configuration line into a Task
fn parse_line(line: &str) -> Option<Task> {
    let line = line.trim();

    // Skip empty lines and comments
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    let parts: Vec<&str> = line.splitn(6, "::").collect();

    // Need at least prefix and command
    if parts.len() < 2 {
        eprintln!(
            "[!] Warning: Invalid config line (needs at least prefix and command): {}",
            line
        );
        return None;
    }

    let task = Task {
        prefix: parts[0].to_string(),
        cmd: parts[1].trim().to_string(),
        suffix: parts.get(2).unwrap_or(&"").to_string(),
        interval: parts
            .get(3)
            .map(|s| s.trim())
            .and_then(|s| s.parse().ok())
            .unwrap_or(1)
            .max(1), // Ensure minimum interval of 1 second
        shell: parts
            .get(4)
            .map(|s| s.trim().eq_ignore_ascii_case("shell"))
            .unwrap_or(false),
        timeout: parts
            .get(5)
            .map(|s| s.trim())
            .and_then(|s| s.parse().ok())
            .filter(|seconds| *seconds > 0),
    };

    if task.is_valid() {
        Some(task)
    } else {
        eprintln!("[!] Warning: Task has empty command: {}", line);
        None
    }
}

/// Loads tasks from config file
fn load_tasks() -> Result<Vec<Task>, BarliError> {
    let config_path = resolve_config_path()?;
    let contents = fs::read_to_string(&config_path).map_err(BarliError::ConfigRead)?;

    let tasks: Vec<Task> = contents.lines().filter_map(parse_line).collect();

    println!(
        "[+] Loaded {} tasks from {}",
        tasks.len(),
        config_path.display()
    );
    Ok(tasks)
}

/// Runs a command and returns its output
fn run_command(task: &Task) -> Result<String, Box<dyn std::error::Error>> {
    let mut command = if task.shell {
        let mut command = Command::new("sh");
        command.arg("-c").arg(&task.cmd);
        command
    } else {
        let parts = shlex::split(&task.cmd).ok_or("[-] Invalid quoted command syntax")?;
        let cmd = parts.first().ok_or("[-] Empty command")?;
        let mut command = Command::new(cmd);
        command.args(parts.iter().skip(1));
        command
    };
    let output = run_with_timeout(&mut command, task.timeout)?;

    if !output.status.success() {
        return Err(format!(
            "[-] Command failed with exit code: {:?}",
            output.status.code()
        )
        .into());
    }

    let result = String::from_utf8(output.stdout)
        .map_err(BarliError::InvalidUtf8)?
        .trim()
        .to_string();

    Ok(result)
}

fn run_with_timeout(
    command: &mut Command,
    timeout_seconds: Option<u64>,
) -> Result<Output, Box<dyn std::error::Error>> {
    if let Some(timeout_seconds) = timeout_seconds {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn()?;

        if child
            .wait_timeout(Duration::from_secs(timeout_seconds))?
            .is_none()
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("[-] Command timed out after {}s", timeout_seconds).into());
        }

        Ok(child.wait_with_output()?)
    } else {
        Ok(command.output()?)
    }
}

/// Worker thread that can be signaled to stop
struct WorkerHandle {
    should_stop: Arc<AtomicBool>,
    join_handle: Option<thread::JoinHandle<()>>,
}

impl WorkerHandle {
    fn new(idx: usize, task: Task, tx: Sender<AppMessage>) -> Self {
        let should_stop = Arc::new(AtomicBool::new(false));
        let should_stop_clone = Arc::clone(&should_stop);

        let join_handle = thread::spawn(move || {
            worker_loop(idx, task, tx, should_stop_clone);
        });

        Self {
            should_stop,
            join_handle: Some(join_handle),
        }
    }

    fn stop(&mut self) {
        self.should_stop.store(true, Ordering::Relaxed);

        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Worker thread loop that runs a task at intervals and can be stopped
fn worker_loop(idx: usize, task: Task, tx: Sender<AppMessage>, should_stop: Arc<AtomicBool>) {
    let mut had_error = false;

    loop {
        if should_stop.load(Ordering::Relaxed) {
            break;
        }

        match run_command(&task) {
            Ok(result) if !result.is_empty() => {
                had_error = false;
                let full = if task.prefix.is_empty() && task.suffix.is_empty() {
                    format!(" {} ", result)
                } else {
                    format!(" {}{}{}", task.prefix, result, task.suffix)
                };

                if tx.send(AppMessage::TaskResult(idx, full)).is_err() {
                    // Main thread has closed, exit worker
                    break;
                }
            }
            Ok(_) => {
                had_error = false;
                // Empty result, send empty string to clear this task's output
                if tx.send(AppMessage::TaskResult(idx, String::new())).is_err() {
                    break;
                }
            }
            Err(e) => {
                if !had_error {
                    eprintln!("[-] Error running task '{}': {}", task.cmd, e);
                    had_error = true;
                }

                let prefix = task.prefix.trim();
                let error_indicator = if prefix.is_empty() {
                    " [ERR] ".to_string()
                } else {
                    format!(" [{}:ERR] ", prefix)
                };

                if tx
                    .send(AppMessage::TaskResult(idx, error_indicator))
                    .is_err()
                {
                    break;
                }
            }
        }

        // Sleep in small chunks so we can respond to stop signals quickly
        let sleep_duration = Duration::from_secs(task.interval);
        let chunk_duration = Duration::from_millis(100);
        let mut remaining = sleep_duration;

        while remaining > Duration::from_millis(0) {
            if should_stop.load(Ordering::Relaxed) {
                return;
            }

            let sleep_time = remaining.min(chunk_duration);
            thread::sleep(sleep_time);
            remaining = remaining.saturating_sub(sleep_time);
        }
    }
}

/// Safely manages X11 display operations
struct DisplayManager {
    display: *mut xlib::Display,
    window: xlib::Window,
}

impl DisplayManager {
    fn new() -> Result<Self, BarliError> {
        unsafe {
            let display = xlib::XOpenDisplay(std::ptr::null());
            if display.is_null() {
                return Err(BarliError::DisplayOpen);
            }

            let window = xlib::XDefaultRootWindow(display);

            Ok(DisplayManager { display, window })
        }
    }

    fn set_window_name(&self, name: &str) -> Result<(), Box<dyn std::error::Error>> {
        let sanitized;
        let name = if name.as_bytes().contains(&0) {
            sanitized = name.replace('\0', "");
            sanitized.as_str()
        } else {
            name
        };

        let c_str = CString::new(name)?;
        unsafe {
            xlib::XStoreName(self.display, self.window, c_str.as_ptr());
            xlib::XSync(self.display, 0);
        }
        Ok(())
    }
}

impl Drop for DisplayManager {
    fn drop(&mut self) {
        unsafe {
            xlib::XCloseDisplay(self.display);
        }
    }
}

/// Sets up signal handling for SIGUSR1 (reload config)
fn setup_signal_handler(tx: Sender<AppMessage>) -> Result<(), BarliError> {
    let mut signals = Signals::new([SIGUSR1, SIGTERM, SIGINT])
        .map_err(|e| BarliError::SignalHandling(e.to_string()))?;

    thread::spawn(move || {
        for signal in signals.forever() {
            let message = match signal {
                SIGUSR1 => AppMessage::ReloadConfig,
                SIGTERM | SIGINT => AppMessage::Shutdown,
                _ => continue,
            };

            if tx.send(message).is_err() {
                break;
            }
        }
    });

    Ok(())
}

/// Main application state
struct App {
    display_manager: DisplayManager,
    workers: Vec<WorkerHandle>,
    results: Vec<String>,
    last_status: String,
}

impl App {
    fn new() -> Result<Self, BarliError> {
        Ok(Self {
            display_manager: DisplayManager::new()?,
            workers: Vec::new(),
            results: Vec::new(),
            last_status: String::new(),
        })
    }

    fn start_workers(&mut self, tasks: Vec<Task>, tx: Sender<AppMessage>) {
        // Stop existing workers
        self.stop_workers();

        // Resize results vector
        self.results = vec![String::new(); tasks.len()];

        // Start new workers
        for (i, task) in tasks.into_iter().enumerate() {
            let worker = WorkerHandle::new(i, task, tx.clone());
            self.workers.push(worker);
        }

        println!("[+] Started {} worker threads", self.workers.len());
    }

    fn stop_workers(&mut self) {
        for worker in &mut self.workers {
            worker.stop();
        }
        self.workers.clear();
        println!("[*] Stopped all worker threads");
    }

    fn handle_task_result(
        &mut self,
        idx: usize,
        text: String,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if idx < self.results.len() {
            self.results[idx] = text;

            let mut status = String::new();
            for part in self.results.iter().filter(|s| !s.is_empty()) {
                if !status.is_empty() {
                    status.push('|');
                }
                status.push_str(part);
            }

            if status != self.last_status {
                self.display_manager.set_window_name(&status)?;
                self.last_status = status;
            }
        }
        Ok(())
    }

    fn reload_config(&mut self, tx: Sender<AppMessage>) -> Result<(), BarliError> {
        println!("[|] Reloading configuration...");
        match load_tasks() {
            Ok(tasks) => {
                if !tasks.is_empty() {
                    self.start_workers(tasks, tx);
                    println!("[*] Configuration reloaded successfully");
                } else {
                    eprintln!("[-] No valid tasks found in config file");
                }
                Ok(())
            }
            Err(e) => {
                eprintln!("[-] Failed to reload config: {}", e);
                Err(e)
            }
        }
    }
}

/// Main update loop that receives messages from worker threads and signal handlers
fn run_app() -> Result<(), Box<dyn std::error::Error>> {
    let (tx, rx) = channel::<AppMessage>();
    let mut app = App::new()?;

    // Set up signal handling
    setup_signal_handler(tx.clone())?;

    // Load initial configuration
    let initial_tasks = load_tasks()?;
    if initial_tasks.is_empty() {
        eprintln!("[-] No valid tasks found in config file");
        return Ok(());
    }

    app.start_workers(initial_tasks, tx.clone());

    println!("[+] Send SIGUSR1 to reload config, SIGTERM/SIGINT to quit.");

    // Main event loop
    while let Ok(message) = rx.recv() {
        match message {
            AppMessage::TaskResult(idx, text) => {
                app.handle_task_result(idx, text)?;
            }
            AppMessage::ReloadConfig => {
                if let Err(e) = app.reload_config(tx.clone()) {
                    eprintln!("Config reload failed: {}", e);
                }
            }
            AppMessage::Shutdown => {
                println!("Received shutdown signal, exiting gracefully...");
                app.stop_workers();
                break;
            }
        }
    }

    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Err(e) = run_app() {
        eprintln!("Application error: {}", e);
        return Err(e);
    }

    Ok(())
}
