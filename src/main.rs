use std::{
    env,
    ffi::CString,
    fs,
    path::PathBuf,
    process::Command,
    sync::mpsc::{channel, Receiver, Sender},
    thread,
    time::Duration,
};
use x11::xlib;

#[derive(Debug, Clone)]
struct Task {
    prefix: String,
    cmd: String,
    suffix: String,
    interval: u64,
    shell: bool,
}

#[derive(Debug)]
enum BarliError {
    ConfigRead(std::io::Error),
    ConfigWrite(std::io::Error),
    DisplayOpen,
    InvalidUtf8(std::string::FromUtf8Error),
}

impl std::fmt::Display for BarliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BarliError::ConfigRead(e) => write!(f, "Failed to read config: {}", e),
            BarliError::ConfigWrite(e) => write!(f, "Failed to write config: {}", e),
            BarliError::DisplayOpen => write!(f, "Failed to open X11 display"),
            BarliError::InvalidUtf8(e) => write!(f, "Invalid UTF-8 in command output: {}", e),
        }
    }
}

impl std::error::Error for BarliError {}

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
    
    let parts: Vec<&str> = line.splitn(5, "::").collect();
    
    // Need at least prefix and command
    if parts.len() < 2 {
        eprintln!("Warning: Invalid config line (needs at least prefix and command): {}", line);
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
    };
   
    if task.is_valid() {
        Some(task)
    } else {
        eprintln!("Warning: Task has empty command: {}", line);
        None
    }
}

/// Runs a command and returns its output
fn run_command(task: &Task) -> Result<String, Box<dyn std::error::Error>> {
    let output = if task.shell {
        Command::new("sh")
            .arg("-c")
            .arg(&task.cmd)
            .output()?
    } else {
        let mut parts = task.cmd.split_whitespace();
        let cmd = parts.next().ok_or("Empty command")?;
        let args: Vec<&str> = parts.collect();
        Command::new(cmd).args(&args).output()?
    };
    
    if !output.status.success() {
        return Err(format!("Command failed with exit code: {:?}", output.status.code()).into());
    }
    
    let result = String::from_utf8(output.stdout)
        .map_err(BarliError::InvalidUtf8)?
        .trim()
        .to_string();
    
    Ok(result)
}

/// Worker thread loop that runs a task at intervals
fn worker_loop(idx: usize, task: Task, tx: Sender<(usize, String)>) {
    loop {
        match run_command(&task) {
            Ok(result) if !result.is_empty() => {
                let full = if task.prefix.is_empty() && task.suffix.is_empty() {
                    format!(" {} ", result)
                } else {
                    format!(" {}{}{}", task.prefix, result, task.suffix)
                };
                
                if tx.send((idx, full)).is_err() {
                    // Main thread has closed, exit worker
                    break;
                }
            }
            Ok(_) => {
                // Empty result, send empty string to clear this task's output
                if tx.send((idx, String::new())).is_err() {
                    break;
                }
            }
            Err(e) => {
                eprintln!("Error running task '{}': {}", task.cmd, e);
                // Send error indicator or empty string
                if tx.send((idx, format!(" [{}:ERR] ", task.prefix))).is_err() {
                    break;
                }
            }
        }
        
        thread::sleep(Duration::from_secs(task.interval));
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

/// Main update loop that receives results from worker threads
fn run_update_loop(
    rx: Receiver<(usize, String)>,
    num_tasks: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let display_manager = DisplayManager::new()?;
    let mut results = vec![String::new(); num_tasks];
    
    while let Ok((idx, text)) = rx.recv() {
        if idx < results.len() {
            results[idx] = text;
            
            let status = results
                .iter()
                .filter(|s| !s.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join("|");
            
            display_manager.set_window_name(&status)?;
        }
    }
    
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config_path = resolve_config_path()?;
    
    let contents = fs::read_to_string(&config_path)
        .map_err(BarliError::ConfigRead)?;
    
    let tasks: Vec<Task> = contents
        .lines()
        .filter_map(parse_line)
        .collect();
    
    if tasks.is_empty() {
        eprintln!("No valid tasks found in config file: {}", config_path.display());
        return Ok(());
    }
    
    println!("Loaded {} tasks from {}", tasks.len(), config_path.display());
    
    let (tx, rx) = channel();
    
    let num_tasks = tasks.len();
    // Spawn worker threads
    for (i, task) in tasks.into_iter().enumerate() {
        let tx_clone = tx.clone();
        thread::spawn(move || worker_loop(i, task, tx_clone));
    }
    
    // Drop the main sender so the receiver will exit when all workers are done
    drop(tx);
    
    // Run the main update loop
    if let Err(e) = run_update_loop(rx, num_tasks) {
        eprintln!("Error in update loop: {}", e);
        return Err(e);
    }
    
    Ok(())
}
