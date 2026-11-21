# Key2PC

Key2PC lets you use your laptop keyboard to control a Windows PC wirelessly over the same local network with millisecond-level latency. It runs quietly in the Windows system tray with no GUI window and reads all settings from `config.json`.

## Features

- Low-latency binary UDP protocol with minimal overhead
- Keyed BLAKE3 packet authentication and fast ACK retransmission to prevent dropped or stuck keys
- Automatic LAN discovery or direct IP targeting
- Background Windows system tray icon showing connection state, active mode, and live RTT latency
- Full configuration via `config.json` with live reload from the tray menu

## Quick Start

1. Download `key2pc.exe` (or `key2pc-windows-x86_64.zip`) from the Releases page, or build it from source.
2. Put `key2pc.exe` and `config.json` in the same folder on both your laptop and your PC.
3. On your PC, open `config.json`, change `"role"` to `"receiver"`, and launch `key2pc.exe`. Allow UDP access through Windows Defender Firewall if prompted.
4. On your laptop, keep `"role": "sender"` and `"target_host": "auto"` (or set `"target_host"` to your PC's local IP address), then launch `key2pc.exe`.
5. Make sure `"shared_secret"` under `"network"` is identical on both devices.

## Usage

- Press `Ctrl+Alt+K` (or right-click the tray icon and click **Forward Keyboard to PC**) to switch keyboard input between your laptop and your PC.
- When forwarding is active, the tray icon turns green and keystrokes go only to the PC. When inactive, the tray icon is blue and typing stays on your laptop.
- Press `Ctrl+Alt+Esc` at any time to release all keys currently held on the PC.
- After editing `config.json`, right-click the tray icon and choose **Reload config.json** to apply your changes without restarting.
- To type into elevated administrator windows on the PC, run `key2pc.exe` as administrator on the PC.

## Configuration

| Option | Description | Default |
| --- | --- | --- |
| `role` | `"sender"` on the laptop or `"receiver"` on the PC | `"sender"` |
| `network.port` | UDP port used for keyboard traffic | `24800` |
| `network.target_host` | `"auto"` for LAN discovery, or your PC IP/hostname | `"auto"` |
| `network.discovery_port` | UDP port used for automatic LAN discovery | `24801` |
| `network.shared_secret` | Shared authentication key that must match on both devices | `"key2pc-change-this-secret"` |
| `network.retransmit_ms` | Fast retry interval when an ACK is not received | `3` |
| `network.disconnect_release_ms` | Auto-release timeout on the PC if connection drops | `1500` |
| `sender.start_active` | Start forwarding to the PC immediately on launch | `false` |
| `sender.toggle_hotkey` | Hotkey combination to toggle laptop vs PC input | `Ctrl+Alt+K` |
| `sender.release_all_hotkey` | Hotkey combination to release all remote keys | `Ctrl+Alt+Escape` |
| `receiver.allow_only_ip` | Optional laptop IP filter (`null` allows any authenticated peer) | `null` |
| `receiver.prefer_scancode` | Send hardware scan-code flags for games that require them | `false` |

## Building from Source

```powershell
cargo test --workspace
cargo build --release --workspace
```

## License

This project is licensed under the [MIT License](LICENSE).
