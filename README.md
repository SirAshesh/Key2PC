# Key2PC

Key2PC turns your Windows laptop into a low-latency wireless keyboard, mouse, and second screen display for your Windows PC over the same local Wi-Fi or LAN connection. It runs in the Windows system tray, stores all settings in `config.json`, and supports hardware-accelerated 1080p 60fps video streaming.

## Features

- Low-latency binary UDP keyboard and mouse forwarding with keyed BLAKE3 authentication
- Fast ACK retransmission and automatic release protection to prevent dropped or stuck keys
- Hardware-accelerated 1080p 60fps display streaming using DXGI Desktop Duplication, Direct3D 11, and Media Foundation H.264
- Automatic secondary monitor selection when a second physical or virtual display is attached on the PC
- Borderless fullscreen viewer mode (`F11`) on the laptop with automatic focus-based input control
- Background Windows system tray icon with live connection status, RTT latency, and config reload

## Quick Start

1. Download `key2pc.exe` (or `key2pc-windows-x86_64.zip`) from the Releases page, or build from source.
2. Put `key2pc.exe` and `config.json` in the same folder on both your PC and your laptop.
3. On your PC, set `"role": "receiver"` in `config.json` and launch `key2pc.exe`. Allow access through Windows Defender Firewall when prompted.
4. On your laptop, keep `"role": "sender"` and `"target_host": "auto"` (or enter your PC's local IP address), then launch `key2pc.exe`.
5. Make sure `"shared_secret"` under `"network"` matches on both devices.

## Using Your Laptop as an Extended Second Screen

When `"display.enabled"` is `true` and `"display.monitor"` is `"auto"`:

- If your PC already has a second monitor, an HDMI dummy plug, or an installed Windows Virtual Display Driver (such as Parsec VDD or Virtual Display Driver), Key2PC automatically streams that secondary monitor as an independent extended screen.
- If your PC only has one monitor, Key2PC streams your primary monitor so you can still view and control the PC immediately.
- Press `Ctrl+Alt+D` (or click **Show Second Screen** in the tray menu) to show or hide the viewer window on your laptop.
- Press `F11` inside the viewer window to toggle fullscreen.

## Controls

- `Ctrl+Alt+K`: Toggle keyboard forwarding between your laptop and PC
- `Ctrl+Alt+D`: Show or hide the Second Screen viewer window
- `Ctrl+Alt+Esc`: Release all held keys and mouse buttons on the PC
- `F11` (in viewer): Toggle borderless fullscreen
- Clicking the viewer window automatically focuses keyboard and mouse input onto the PC when `"auto_forward_on_viewer_focus"` is `true`

## Configuration

| Option | Description | Default |
| --- | --- | --- |
| `role` | `"sender"` on the laptop or `"receiver"` on the PC | `"sender"` |
| `network.port` | UDP port for keyboard, mouse, and control packets | `24800` |
| `network.target_host` | `"auto"` for LAN discovery, or your PC IP/hostname | `"auto"` |
| `network.discovery_port` | UDP port for automatic LAN discovery | `24801` |
| `network.shared_secret` | Shared authentication key that must match on both devices | `"key2pc-change-this-secret"` |
| `display.enabled` | Enable 1080p 60fps second screen streaming | `true` |
| `display.video_port` | UDP port for streamed H.264 video frames | `24802` |
| `display.monitor` | `"auto"` (prefers monitor 2) or monitor index such as `"0"` or `"1"` | `"auto"` |
| `display.width` | Stream width in pixels | `1920` |
| `display.height` | Stream height in pixels | `1080` |
| `display.fps` | Target frame rate | `60` |
| `display.bitrate_kbps` | Target H.264 bitrate in kbps | `16000` |
| `display.fullscreen` | Open viewer directly in fullscreen | `false` |
| `display.open_viewer_on_start` | Open the viewer window automatically on laptop launch | `true` |
| `display.capture_mouse` | Forward mouse moves, clicks, and scroll wheel from viewer | `true` |
| `display.auto_forward_on_viewer_focus` | Automatically forward keyboard when viewer window has focus | `true` |
| `sender.start_active` | Start keyboard forwarding immediately on launch | `false` |
| `receiver.allow_only_ip` | Optional laptop IP filter (`null` allows any authenticated peer) | `null` |
| `receiver.prefer_scancode` | Send hardware scan-code flags for games | `false` |

## Building from Source

```powershell
cargo test --workspace
cargo build --release --workspace
```

## License

This project is licensed under the [MIT License](LICENSE).
