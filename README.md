# Antigravity Route

A standalone, high-performance proxy daemon written in Rust that converts your Google Antigravity (Code Assist) subscription into an OpenAI-compatible API. This allows you to use powerful models like Claude 3.5 Sonnet and Gemini 1.5 Pro (via Google's infrastructure) in any AI agent or application that expects standard OpenAI endpoints.

## ⚠️ DISCLAIMER & WARNING

**IMPORTANT: Use this software at your own risk.**

This project reverse-engineers and proxies the internal APIs used by Google Cloud Code / Gemini Code Assist. Using this software to access these models outside of the official IDE extensions **is likely a violation of Google's Terms of Service (TOS)**. 

Google may actively monitor for abnormal usage patterns (such as non-IDE User-Agents, excessive request rates, or unsupported endpoints). Abuse of this system **can and may result in the suspension or permanent termination of your Google Account**, Google Cloud organization, or Antigravity subscription without warning. The authors of this tool hold absolutely no liability for any damage, bans, or loss of data incurred by its use.

## Features

- **OpenAI Compatibility**: Exposes a `/v1/chat/completions` endpoint that perfectly mimics OpenAI's API.
- **Smart Model Mapping**: Automatically translates standard model names (e.g., `claude-3-5-sonnet-latest`, `gpt-4o`) to the specific backend models provisioned by Antigravity (`claude-sonnet-4-6-thinking`, `gemini-1.5-pro` etc).
- **True Streaming Support**: Fully supports HTTP Server-Sent Events (SSE) streaming for real-time typewriter-like token generation, piping bytes asynchronously with zero intermediate buffering.
- **Message Translation**: Robustly converts OpenAI's `messages` array into Gemini's `contents` format, including system prompt remapping, alternating role merging, and base64 image (multimodal) extraction.
- **Visual Quota CLI**: Includes a built-in terminal command to securely fetch and display your remaining Weekly and 5-Hour limits using beautiful ANSI colored progress bars.
- **Standalone Daemon**: Designed as a C/S architecture. Runs silently in the background, storing all auth state locally in a customizable directory instead of clobbering your home folder.

## Installation

### Using Nix (Recommended)

If you use the Nix package manager, you can run the daemon directly without needing to install Rust or compile anything manually:

```bash
nix run . -- daemon --datadir /var/lib/antigravity-route
# Or directly from GitHub:
# nix run github:CHN-beta/antigravity-route -- daemon --datadir /var/lib/antigravity-route
```

### Using Cargo

Ensure you have [Rust and Cargo](https://rustup.rs/) installed, then build the release binary:

```bash
cargo build --release
```

The executable will be located at `target/release/antigravity-route`. You can move this to your `$PATH`.

## Usage

The application provides three main commands: `daemon`, `auth`, and `quota`.

### 1. Start the Daemon

First, start the background server. You must specify a directory where your authentication tokens will be securely saved.

```bash
antigravity-route daemon --datadir /var/lib/antigravity-route --port 8999
```

*(You can set the `RUST_LOG=info` or `RUST_LOG=debug` environment variable to see detailed server logs.)*

### 2. Authenticate

In a separate terminal window, initiate the OAuth login process. This communicates with your running daemon.

```bash
antigravity-route auth --daemon-url http://127.0.0.1:8999
```

The CLI will print a Google OAuth URL. 
1. Open this URL in your web browser.
2. Sign in with the Google Account that has an active Antigravity/Code Assist subscription.
3. Grant the required permissions.
4. You will be redirected to a URL on `localhost`. Copy the `code=...` parameter from the address bar (or paste the entire redirected URL) back into your terminal.

The daemon will securely exchange this for long-lived tokens and save them to the directory you specified in Step 1.

### 3. Check Quotas

At any time, you can view your live API limits:

```bash
antigravity-route quota --daemon-url http://127.0.0.1:8999
```

This will print a formatted, color-coded progress bar showing exactly how much of your 5-Hour and Weekly limits you have consumed across Gemini and Claude model groups.

## Using with Agents

Once the daemon is running and authenticated, you can point any OpenAI-compatible agent (like Continue.dev, Cursor, Aider, or custom scripts) to your local server:

- **Base URL**: `http://127.0.0.1:8999/v1`
- **API Key**: `sk-anything` (The proxy handles auth natively, so any placeholder key works)
- **Model**: Use standard names like `claude-3-5-sonnet-latest` or `gemini-1.5-pro`

## License
MIT
