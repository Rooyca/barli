# barli

**barli** is a tiny, lightweight status bar for X11.  
It runs shell commands (or plain executables) at given intervals and updates the X11 root window name with their results — perfect for minimal WMs like `dwm` or `xmonad`.

---

## Features
- Simple text-based configuration file (`~/.config/barli.conf` or `~/.barli.conf`).  
- Hot-reload configuration by sending `SIGUSR1` to the process (no restart required).
- Runs commands periodically and displays their output.  
- Supports both plain commands and full shell commands.  
- Lightweight, written in Rust, with no external daemons.  

---

## Configuration

Each line in the config defines a task:

```
prefix :: command :: suffix :: interval :: [shell] :: [timeout_seconds]
````

- **prefix** → Text shown before command output  
- **command** → The command to run  
- **suffix** → Text shown after command output  
- **interval** → Update interval (seconds)  
- **shell** → Optional, set to `shell` to run inside `/bin/sh -c`  
- **timeout_seconds** → Optional, kills the command if it runs longer than this value  

> ![TIP] After editing your config file, you can reload it without restarting `barli` by running:
> ```bash
> pkill -USR1 barli
> ```

### Example (`~/.config/barli.conf`)
```txt
Clock :: date :: :: 2 :: 
Mem :: free -h | awk 'NR==2{print $3}' :: used :: 10 :: shell :: 2
````

---

## Installation

Use [`just`](https://github.com/casey/just) to build, install, and manage **barli**:

```bash
# build release binary
just build

# install to ~/.local/bin
just install

# uninstall
just uninstall
```

---

## Usage

Just run:

```bash
barli &
```

The bar text is stored in the root window name, so it will automatically be picked up by your WM’s status bar.
