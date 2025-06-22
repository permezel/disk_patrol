# Disk Patrol

A high-performance, multi-threaded disk health monitoring tool for Linux systems. Disk Patrol continuously reads from block devices to detect and report I/O errors, helping identify failing sectors before they cause data loss.

## Features

- **Continuous Monitoring**: Patrols entire disk surfaces over configurable time periods
- **Multi-Device Support**: Monitor multiple disks concurrently with independent patrol cycles
- **Smart Scheduling**: Adaptive sleep intervals with optional jitter to spread I/O load
- **Error Detection**: Tracks and reports read errors with sector-level precision
- **Email Alerts**: Configurable SMTP notifications when error thresholds are exceeded
- **Progress Tracking**: Real-time progress bars showing patrol status and error counts
- **State Persistence**: Resumes patrol from last position after restarts
- **O_DIRECT I/O**: Bypasses page cache for accurate hardware error detection
- **Flexible Logging**: Support for file logging and syslog integration

## Installation

### From Source

```bash
# Install Rust if not already installed
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Clone and build
git clone https://github.com/yourusername/disk-patrol.git
cd disk-patrol
cargo build --release

# Install binary
sudo cp target/release/disk_patrol /usr/local/bin/

# Create directories
sudo mkdir -p /etc/disk_patrol /var/lib/disk_patrol /var/log
```

### Dependencies

- Rust 1.70 or later
- Linux kernel with O_DIRECT support
- Block device access (typically requires root)

## Quick Start

1. Generate a configuration file:
```bash
disk_patrol --generate-config /etc/disk_patrol/config.toml
```

2. Edit the configuration to specify your devices:
```bash
sudo nano /etc/disk_patrol/config.toml
```

3. Start patrolling:
```bash
sudo disk_patrol --config /etc/disk_patrol/config.toml
```

## Configuration

### Configuration File (TOML)

```toml
# Devices to patrol
devices = ["/dev/sda", "/dev/nvme0n1"]

# Complete patrol cycle duration (days)
patrol_period_days = 30

# Read size per operation (supports KB/MB suffixes)
read_size = "8MB"

# State persistence
state_file = "/var/lib/disk_patrol/state.json"

# Logging
log_file = "/var/log/disk_patrol.log"
verbose = false
use_syslog = true
syslog_facility = "daemon"

# Email alerts
email_alerts = true
smtp_server = "smtp.example.com"
smtp_port = 587
smtp_use_starttls = true
smtp_username = "disk_patrol@example.com"
smtp_password = "your_password_here"
alert_from = "disk_patrol@example.com"
alert_to = ["admin@example.com", "ops@example.com"]
error_threshold = 5

# Performance tuning
show_progress = true
enable_jitter = true
max_jitter_percent = 25
```

### Command Line Options

```bash
disk_patrol [OPTIONS] [DEVICES]...

OPTIONS:
    -c, --config <FILE>              Configuration file path [default: /etc/disk_patrol/config.toml]
    -p, --period <DAYS>              Patrol period in days [default: 30]
    -r, --read-size <BYTES>          Read size per operation [default: 8MB]
    -s, --state-file <FILE>          State file path [default: /var/lib/disk_patrol/state.json]
    -v, --verbose                    Enable verbose output
        --status                     Show patrol status and exit
        --reset                      Reset patrol state for specified devices
        --reset-all                  Reset state for all devices
        --generate-config <FILE>     Generate example configuration file
        --merge-config               Merge config file with command line args
        --test-email                 Send test email and exit
        --progress                   Show progress bars
        --enable-jitter              Enable timing jitter
        --max-jitter <PERCENT>       Maximum jitter percentage [default: 25]

LOGGING:
        --syslog                     Enable syslog logging
        --syslog-facility <FACILITY> Syslog facility [default: daemon]

EMAIL ALERTS:
        --email-alerts               Enable email alerts
        --smtp-server <SERVER>       SMTP server hostname
        --smtp-port <PORT>           SMTP server port [default: 587]
        --smtp-starttls              Use STARTTLS
        --smtp-username <USERNAME>   SMTP username
        --smtp-password <PASSWORD>   SMTP password
        --alert-from <EMAIL>         Alert sender address
        --alert-to <EMAIL>           Alert recipient (can specify multiple)
        --error-threshold <COUNT>    Errors before alert [default: 5]
```

## Usage Examples

### Basic Usage

```bash
# Patrol a single disk
sudo disk_patrol /dev/sda

# Patrol multiple disks
sudo disk_patrol /dev/sda /dev/sdb /dev/nvme0n1

# Use configuration file
sudo disk_patrol --config /etc/disk_patrol/config.toml
```

### Status and Management

```bash
# Check patrol status
sudo disk_patrol --config /etc/disk_patrol/config.toml --status

# Reset patrol state for specific device
sudo disk_patrol --config /etc/disk_patrol/config.toml --reset /dev/sda

# Reset all device states
sudo disk_patrol --config /etc/disk_patrol/config.toml --reset-all
```

### Testing and Debugging

```bash
# Test email configuration
sudo disk_patrol --config /etc/disk_patrol/config.toml --test-email

# Run with verbose output
sudo disk_patrol --config /etc/disk_patrol/config.toml --verbose --progress
```

## How It Works

### Patrol Algorithm

1. **Initialization**: Reads device sizes and loads previous state
2. **Scheduling**: Calculates optimal read intervals based on device size and patrol period
3. **Reading**: Performs O_DIRECT reads at calculated positions
4. **Error Tracking**: Records any I/O errors with timestamp and sector information
5. **Progress**: Updates position and saves state periodically
6. **Alerting**: Sends email notifications when error thresholds are exceeded

### Timing and Jitter

To prevent synchronized I/O spikes when monitoring multiple devices, Disk Patrol supports timing jitter:

- **Base Interval**: `patrol_period / (device_size / read_size)`
- **Jitter**: Random variation up to `max_jitter_percent` of base interval
- **Distribution**: Jitter is randomly split between pre-read and post-read delays

### Memory Efficiency

Disk Patrol uses a single shared buffer for all devices, minimizing memory usage while maintaining high performance. The buffer is aligned for O_DIRECT operations and sized according to the configured read size.

## Systemd Service

Create `/etc/systemd/system/disk-patrol.service`:

```ini
[Unit]
Description=Disk Patrol - Continuous disk health monitoring
After=network.target

[Service]
Type=simple
ExecStart=/usr/local/bin/disk_patrol --config /etc/disk_patrol/config.toml
Restart=on-failure
RestartSec=30
User=root
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
```

Enable and start:
```bash
sudo systemctl enable disk-patrol
sudo systemctl start disk-patrol
```

## Monitoring Integration

### Prometheus Metrics (Future)

Disk Patrol can expose metrics for Prometheus monitoring:
- `disk_patrol_errors_total`: Total errors per device
- `disk_patrol_progress_percent`: Patrol progress percentage
- `disk_patrol_last_error_time`: Timestamp of last error

### Log Analysis

Disk Patrol logs can be parsed for monitoring:
```bash
# Count errors per device
grep "I/O Error" /var/log/disk_patrol.log | awk '{print $4}' | sort | uniq -c

# Recent errors
journalctl -u disk-patrol --since "1 hour ago" | grep ERROR
```

## Performance Considerations

- **Read Size**: Larger reads (8-16MB) are more efficient but may impact system responsiveness
- **Patrol Period**: Longer periods reduce I/O load but increase time to detect errors
- **Jitter**: Helps prevent I/O storms when multiple devices complete reads simultaneously
- **O_DIRECT**: Bypasses cache but requires aligned buffers and may not work on all filesystems

## Troubleshooting

### Common Issues

1. **Permission Denied**: Run with sudo or as root
2. **Device Not Found**: Check device path exists and is a block device
3. **Email Not Sending**: Verify SMTP settings and test with `--test-email`
4. **High I/O Impact**: Increase patrol period or reduce read size

### Debug Commands

```bash
# Check device accessibility
ls -la /dev/sda
lsblk

# Verify state file
cat /var/lib/disk_patrol/state.json | jq .

# Test specific device
sudo disk_patrol /dev/sda --verbose --period 1
```

## Contributing

Contributions are welcome! Please:

1. Fork the repository
2. Create a feature branch
3. Add tests for new functionality
4. Ensure all tests pass
5. Submit a pull request

## License

This project is licensed under the MIT License - see the LICENSE file for details.

## Acknowledgments

- Inspired by the need for proactive disk failure detection
- Built with Rust for performance and reliability
- Uses tokio for async I/O and concurrency
- Developed in conjunction with Claude 4.0

## Roadmap

- [ ] SMART data integration
- [ ] Prometheus metrics exporter
- [ ] Web dashboard for status monitoring
- [ ] Support for network-attached storage
- [ ] Configurable read patterns (sequential, random)
- [ ] Machine learning for failure prediction
patrol read for hard drive health
