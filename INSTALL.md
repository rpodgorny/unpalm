# Installation Guide

## Quick Start

### 1. Test the filter manually (requires root)
```bash
sudo ./target/release/unpalm
```

Check that it finds your touchpad and creates the filtered device. Try touching the edges - those touches should be blocked.

Press Ctrl+C to stop.

### 2. Install as a system service

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

You can customize margins and add polygon zones using CLI arguments. Edit the systemd service file to add your preferences:

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
sudo unpalm -n "*YourTouchpadName*"
# or
sudo unpalm -f /dev/input/eventX
```

**Permission denied**: The filter needs root access to grab input devices. The systemd service runs as root automatically. If running manually, use `sudo`.

**Service fails to start**:
- Check logs: `journalctl -u unpalm.service -n 50`
- Verify the touchpad device exists: `ls -l /dev/input/by-path/*event*`
- For external/Bluetooth touchpads, ensure the device is connected when the service starts
- You may need to add a delay in the service file: `ExecStartPre=/bin/sleep 2`
