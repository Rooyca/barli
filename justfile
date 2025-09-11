# Build the project in release mode
build:
    cargo build --release

dev:
    cargo run

# Install binary to ~/.local/bin
install: build
    mkdir -p ~/.local/bin
    cp target/release/barli ~/.local/bin/

# Uninstall binary
uninstall:
    rm -f ~/.local/bin/barli

# Clean build artifacts
clean:
    cargo clean

# Run barli in the background
run: build
    ~/.local/bin/barli &
