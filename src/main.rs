use std::{
    env,
    ffi::CString,
    fs,
    path::PathBuf,
    process::Command,
    sync::mpsc::{channel, Sender},
    thread,
    time::Duration,
};
use x11::xlib;
use std::ptr;

#[derive(Debug, Clone)]
struct Task {
    prefix: String,
    cmd: String,
    suffix: String,
    interval: u64,
    shell: bool,
}

fn resolve_config_path() -> PathBuf {
    let home = env::var("HOME").unwrap_or_else(|_| ".".into());
    let config_dir = PathBuf::from(&home).join(".config/barli.conf");
    let legacy = PathBuf::from(&home).join(".barli.conf");

    if config_dir.exists() {
        config_dir
    } else if legacy.exists() {
        legacy
    } else {
        // create a simple default config
        let default = "Clock :: date :: :: 1 ::  \n";
        fs::write(&config_dir, default).unwrap();
        config_dir
    }
}

fn parse_line(line: &str) -> Option<Task> {
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    let parts: Vec<&str> = line.splitn(5, "::").map(|s| s.trim()).collect();
        if parts.is_empty() {
            return None;
        }

        Some(Task {
            prefix: parts.get(0).unwrap_or(&"").to_string(),
            cmd:    parts.get(1).unwrap_or(&"").to_string(),
            suffix: parts.get(2).unwrap_or(&"").to_string(),
            interval: parts
                .get(3)
                .and_then(|s| s.parse().ok())
                .unwrap_or(1),
            shell: parts
                .get(4)
                .map(|s| s.eq_ignore_ascii_case("shell"))
                .unwrap_or(false),
        })
   }

fn run_command(task: &Task) -> Option<String> {
    let output = if task.shell {
        Command::new("sh")
            .arg("-c")
            .arg(&task.cmd)
            .output()
            .ok()?
    } else {
        let mut parts = task.cmd.split_whitespace();
        let cmd = parts.next()?;
        let args: Vec<&str> = parts.collect();
        Command::new(cmd).args(&args).output().ok()?
    };

    if !output.status.success() {
        return None;
    }

    let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

fn worker_loop(idx: usize, task: Task, tx: Sender<(usize, String)>) {
    loop {
        if let Some(result) = run_command(&task) {
            let full = format!(" {} {}{} ", task.prefix, result, task.suffix);
            let _ = tx.send((idx, full));
        }
        thread::sleep(Duration::from_secs(task.interval));
    }
}

fn main() {
    let config_path = resolve_config_path();
    let contents = fs::read_to_string(&config_path).unwrap_or_default();
    let tasks: Vec<Task> = contents
        .lines()
        .filter_map(|line| parse_line(line))
        .collect();

    let (tx, rx) = channel();

    // spawn workers
    for (i, task) in tasks.iter().cloned().enumerate() {
        let tx = tx.clone();
        thread::spawn(move || worker_loop(i, task, tx));
    }

    unsafe {
        let display = xlib::XOpenDisplay(ptr::null());
        if display.is_null() {
            eprintln!("Failed to open X11 display");
            return;
        }
        let window = xlib::XDefaultRootWindow(display);
        let mut results = vec![String::new(); tasks.len()];

        // update loop
        while let Ok((idx, text)) = rx.recv() {
            results[idx] = text;
            let status = results
                .iter()
                .filter(|s| !s.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join("|");
            if let Ok(c_str) = CString::new(status) {
                xlib::XStoreName(display, window, c_str.as_ptr());
                xlib::XSync(display, 0);
            }
        }

        xlib::XCloseDisplay(display);
    }
}
