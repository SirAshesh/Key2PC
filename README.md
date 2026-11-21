# Key2PC

Key2PC turns your Windows laptop into a low-latency wireless keyboard, mouse, and second screen display for your Windows PC over the same local Wi-Fi or LAN connection. It runs in the Windows system tray, stores all settings in `config.json`, and supports hardware-accelerated 1080p 60fps video streaming with virtual extended monitor support.

## Features

- Low-latency binary UDP keyboard and mouse forwarding with keyed BLAKE3 authentication
- Fast ACK retransmission and automatic release protection to prevent dropped or stuck keys
- Hardware-accelerated 60fps display streaming using DXGI Desktop Duplication, Direct3D 11, and Media Foundation H.264 with live Windows cursor compositing
- Automatic native source aspect matching (such as `1920x1200` 16:10 or `1920x1080` 16:9) without unexpected letterboxing
- Built-in virtual monitor integration on the PC (supports Parsec Virtual Display Adapter and Virtual Display Driver) to extend your desktop onto your laptop without needing a physical second screen
- One-click Virtual Monitor Driver installer (`--install-vdd` or tray menu) with Windows UAC verification
- Flexible fullscreen (`F11`, `Alt+Enter`, `Ctrl+Alt+F`) and zero-bars fill scaling (`F10`, tray menu, or `display.fill_screen`) on the laptop viewer
- Background Windows system tray icon with live connection status, RTT latency, monitor mode, and config reload

## Quick Start

1. Download `key2pc.exe` (or `key2pc-windows-x86_64.zip`) from the Releases page, or build from source.
2. Put `key2pc.exe` and `config.json` in the same folder on both your PC and your laptop.
3. On your PC, set `"role": "receiver"` in `config.json` and launch `key2pc.exe`. Allow access through Windows Defender Firewall when prompted.
4. On your laptop, keep `"role": "sender"` and `"target_host": "auto"` (or enter your PC's local IP address), then launch `key2pc.exe`.
5. Make sure `"shared_secret"` under `"network"` matches on both devices.

## Extending Your PC Desktop to Your Laptop

1. Launch `key2pc.exe` on your PC with `"role": "receiver"`.
2. Right-click or click the Key2PC tray icon on your PC and choose **Install Virtual Monitor Driver (UAC)...** (or run `key2pc.exe --install-vdd`). Approve the Windows UAC prompt to install the signed virtual display driver once.
3. With `"display.virtual_monitor": true`, Key2PC automatically attaches a virtual second monitor (`1920x1080 @ 60Hz` by default), switches Windows into Extend mode, and streams that second monitor to your laptop.
4. Use **PC Virtual Extended Monitor** in the PC tray menu anytime to switch between Extend mode and Mirror mode without restarting. The temporary virtual monitor is automatically unplugged when Key2PC exits.

## Controls

- `Ctrl+Alt+K`: Toggle keyboard forwarding between your laptop and PC
- `Ctrl+Alt+D`: Show or hide the Second Screen viewer window
- `Ctrl+Alt+F`, `F11`, or `Alt+Enter`: Toggle borderless fullscreen on the Second Screen viewer window (also selectable from the tray menu)
- `F10`: Toggle `Fill Screen Without Bars` stretch mode on the Second Screen viewer window (also selectable from the tray menu)
- `Ctrl+Alt+Esc`: Release all held keys and mouse buttons on the PC
- Clicking the viewer window automatically focuses keyboard and mouse input onto the PC when `"auto_forward_on_viewer_focus"` is `true`

## Configuration

| Option | Description | Default |
| --- | --- | --- |
| `role` | `"sender"` on the laptop or `"receiver"` on the PC | `"sender"` |
| `network.port` | UDP port for keyboard, mouse, and control packets | `24800` |
| `network.target_host` | `"auto"` for LAN discovery, or your PC IP/hostname | `"auto"` |
| `network.discovery_port` | UDP port for automatic LAN discovery | `24801` |
| `network.shared_secret` | Shared authentication key that must match on both devices | `"key2pc-change-this-secret"` |
| `display.enabled` | Enable 60fps second screen streaming | `true` |
| `display.virtual_monitor` | Automatically create a virtual extended second monitor on the PC | `true` |
| `display.video_port` | UDP port for streamed H.264 video frames | `24802` |
| `display.monitor` | `"auto"` (prefers virtual or secondary monitor) or monitor index such as `"0"` or `"1"` | `"auto"` |
| `display.width` | Preferred virtual monitor width or fallback encode width | `1920` |
| `display.height` | Preferred virtual monitor height or fallback encode height | `1080` |
| `display.match_source_resolution` | Encode at the captured monitor's native aspect ratio and resolution (up to 2560x1600) | `true` |
| `display.fps` | Target frame rate | `60` |
| `display.bitrate_kbps` | Target H.264 bitrate in kbps | `16000` |
| `display.fullscreen` | Open viewer directly in fullscreen | `false` |
| `display.fill_screen` | Stretch video to fill the entire viewer window without black bars | `false` |
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
