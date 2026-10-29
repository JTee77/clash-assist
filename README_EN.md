# Clash Assist / Clash伴侣

[简体中文](./README.md) | **English**

**Clash Assist** (**Clash伴侣**) is a lightweight macOS (Apple Silicon) desktop companion that **tests how well your proxy nodes actually work and gives each node an easy-to-read star rating**.

Ping numbers inside a proxy client often look great on paper, even when a node is slow, blocked by AI tools, or unable to play streaming video. **Clash Assist** runs real-world checks on your nodes and tells you the results in plain language:

- ⭐ **Instant Star Ratings (1–5 Stars)**: Automatically ranks tested nodes from best to worst so you can spot your most reliable daily-driver nodes at a glance.
- 🤖 **AI & Streaming Unlock Checks**: Tests whether each node works with **ChatGPT** and **Gemini**, and checks unlock status + region for **Netflix, YouTube, Disney+, TikTok, Spotify**, and more.
- 🚀 **Real Download Speed & IP Quality**: Measures actual download bandwidth and latency, identifies the exit country, and infers the IP type from organization info — labeled honestly in the UI as “Likely datacenter (DCH)” or “Residential (ISP)”.
- 🛡️ **Privacy & Leak Check**: One-click check to verify whether your setup is accidentally leaking your real IP or DNS, and whether domestic/overseas split routing works as expected.
- 🔍 **Region Filter & Auto-Saved Results**: Filter by region (e.g., Japan, Hong Kong) or keyword to test only the nodes you care about. Test results are saved automatically so they are ready the next time you open the app.

> **Supported Clients**: **Clash Verge (Clash Verge Rev)** and **FlClash**.  
> *(Note: Clash Assist is a diagnostic companion tool only — it does not provide any proxy nodes or VPN services.)*

---

## Download & Installation

Download the latest `.dmg` installer from [Releases](https://github.com/JTee77/clash-assist/releases) and drag **Clash Assist** into your **Applications** folder.

### Why does macOS say “Apple could not verify Clash Assist” on first launch or after an update?

Because the app is signed locally (ad-hoc) rather than notarized with a paid $99/year Apple Developer ID, macOS Gatekeeper will show a security prompt **the first time you open a newly downloaded or updated version** (subsequent launches of the same version will open normally):

1. Click **“Done”** in the dialog (do not click “Move to Trash”).
2. Open **System Settings › Privacy & Security**, scroll down, and click **“Open Anyway”**.
3. Click **“Open Anyway”** again in the confirmation prompt and authenticate with your Mac password or Touch ID.

> ✨ **Zero-Prompt In-App Updates (v0.2.4+)**: Once installed, whenever a new version is available, simply click **“Update now”** in the top titlebar pill — Clash Assist will download, replace, and restart itself automatically **without triggering Gatekeeper prompts again**.
> 💡 **Quick terminal shortcut (optional)**: On your very first install from a browser, you can also run `xattr -cr "/Applications/Clash Assist.app"` in Terminal to skip System Settings.

### About the “Local Network” Permission Prompt

The first time you run **Environment leak check**, macOS may ask for **Local Network** access. This is used by the DNS and WebRTC leak checks to inspect your local router/system DNS resolver. Node testing works even if you decline.

---

## How to Use

1. Open **Clash Verge** or **FlClash** and connect to a node as usual.
   - *If you use **FlClash**, first enable **“External Controller”** in FlClash under **Settings → General → Inbound**.*
2. Open **Clash Assist** — it automatically detects the client you're running and loads your nodes. If detection fails, choose your client under “Which app do you use?” at the top, then click **“Refresh nodes”**.
3. From there, you can:
   - Click **“Test”** on any single node card to run a full check on that node; or
   - Filter by region/keyword and click **“Test filtered” / “Test all nodes”** — Clash Assist will test them one by one, rate each node, and automatically switch back to your original node when finished.

---

## Command-Line Interface (CLI)

Clash Assist also ships with a built-in CLI that shares cached results with the GUI. Run the following once to install the `clash-assist` command into your `PATH`:

```bash
"/Applications/Clash Assist.app/Contents/MacOS/clash-assist" --cli install-shim
clash-assist --cli help
```

See [docs/CLI.md](./docs/CLI.md) for full documentation (and [docs/AGENTS-CLI.md](./docs/AGENTS-CLI.md) for AI agent integration).

---

## Notes & Disclaimer

- **No proxy service provided**: This tool is strictly for local diagnostics and does not provide any proxy nodes or VPN services.
- **Data usage**: Deep speed tests download a small sample payload through each tested node to measure real bandwidth. Please be mindful of your data plan.
- **For reference only**: Unlock and leak checks are based on real-time sample probes and may vary as target services update their rules.

## Contributing & License

Issues and PRs are welcome! See [CONTRIBUTING.md](./CONTRIBUTING.md) for developer notes.  
Licensed under the [GNU GPL-3.0](./LICENSE).
