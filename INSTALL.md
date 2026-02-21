# Installation Guide

## Install Methods

**From crates.io:**
```bash
cargo install unpalm
```

**Arch Linux (AUR):**
```bash
yay -S unpalm
# or
paru -S unpalm
```

**From source:**
```bash
git clone https://github.com/rpodgorny/unpalm.git
cd unpalm
cargo build --release
sudo cp target/release/unpalm /usr/local/bin/
```

## Setup

### 1. Test the filter manually
```bash
unpalm
# or, if built from source without installing:
./target/release/unpalm
```

Check that it finds your touchpad and creates the filtered device. Try touching the edges - those touches should be blocked.

Press Ctrl+C to stop.

### 2. Install as a user service (recommended)

Since unpalm doesn't need root (just `input` group membership), a systemd user service is the simplest setup:

```bash
# Copy the binary
sudo cp target/release/unpalm /usr/local/bin/

# Install the user service
mkdir -p ~/.config/systemd/user/
cp unpalm.user.service ~/.config/systemd/user/unpalm.service

# Enable and start the service
systemctl --user daemon-reload
systemctl --user enable unpalm.service
systemctl --user start unpalm.service

# Check status
systemctl --user status unpalm.service

# View logs
journalctl --user -u unpalm.service -f
```

If you need the service to run without an active login session (e.g., on a headless machine):
```bash
loginctl enable-linger $USER
```

### Alternative: system service

If you need the service to run as root (e.g., your system lacks the uinput ACL rule for the `input` group):

```bash
# Copy the binary
sudo cp target/release/unpalm /usr/local/bin/

# Install the systemd service
sudo cp unpalm.service /etc/systemd/system/

# Enable and start the service
sudo systemctl daemon-reload
sudo systemctl enable unpalm.service
sudo systemctl start unpalm.service

# Check status
systemctl status unpalm.service

# View logs
journalctl -u unpalm.service -f
```

### 3. Verify the virtual device exists

```bash
libinput list-devices | grep "Filtered Touchpad"
```

## Customizing Exclusion Zones

You can customize margins and add polygon zones using CLI arguments. Edit the service file to add your preferences:

**User service:**
```bash
nano ~/.config/systemd/user/unpalm.service
```

**System service:**
```bash
sudo nano /etc/systemd/system/unpalm.service
```

Modify the `ExecStart` line to include your desired options:

```ini
# Custom margins (30% left/right, 15% top, 10% bottom)
ExecStart=/usr/local/bin/unpalm --margin-left 30 --margin-right 30 --margin-top 15 --margin-bottom 10

# Or with custom polygon zones
ExecStart=/usr/local/bin/unpalm --polygon "0,0 15,0 0,25" --polygon "85,0 100,0 100,25"

# Or specify a specific touchpad by name pattern
ExecStart=/usr/local/bin/unpalm -n "*Synaptics*" --margin-left 25
```

After editing, reload and restart the service:

**User service:**
```bash
systemctl --user daemon-reload
systemctl --user restart unpalm.service
```

**System service:**
```bash
sudo systemctl daemon-reload
sudo systemctl restart unpalm.service
```

See the [README.md](README.md) for all available CLI options and usage examples.

## Troubleshooting

**Device not found**: Check available touchpads:
```bash
libinput list-devices | grep -i touchpad
```

If multiple touchpads are detected, specify the one you want using the `-n` or `-f` option in the service file:
```bash
unpalm -n "*YourTouchpadName*"
# or
unpalm -f /dev/input/eventX
```

**Permission denied**: unpalm needs access to `/dev/input/event*` and `/dev/uinput`. Add your user to the `input` group: `sudo usermod -aG input $USER` (then log out and back in). Alternatively, run with `sudo` or use the system service.

**Service fails to start**:
- Check logs:
  - User service: `journalctl --user -u unpalm.service -n 50`
  - System service: `journalctl -u unpalm.service -n 50`
- Verify the touchpad device exists: `ls -l /dev/input/by-path/*event*`
- For external/Bluetooth touchpads, ensure the device is connected when the service starts
- You may need to add a delay in the service file: `ExecStartPre=/bin/sleep 2`
