# filet

End-to-end encrypted file transfer. No storage, no accounts, direct relay between sender and receiver.

Files are encrypted in the browser using AES-256-GCM before leaving the sender's machine. The encryption key is placed in the URL fragment (`#`), which is never sent to the server. The server only relays encrypted bytes -- it cannot read the file contents.

## How it works

1. **Sender** opens the web interface, selects a file. A 256-bit AES-GCM key is generated in the browser.
2. The sender's browser opens a WebSocket to the server and registers the transfer. The server returns a transfer ID.
3. A download link is generated: `http://localhost:PORT/d/TRANSFER_ID#ENCRYPTION_KEY`. The sender shares this link with the recipient.
4. **Recipient** opens the link. The browser fetches transfer metadata (filename, size) from the server API, then connects via WebSocket.
5. The server signals the sender to begin. The sender reads the file in chunks, encrypts each chunk with AES-GCM (unique IV per chunk), and sends the ciphertext over WebSocket.
6. The server relays each encrypted chunk to the recipient's WebSocket in real time. Nothing is written to disk.
7. The recipient's browser decrypts each chunk using the key from the URL fragment, assembles the file, and triggers a download.

The transfer is strictly one-to-one. Once a recipient connects, the transfer is claimed. If either side disconnects, the transfer is cancelled.

## Requirements

- Rust 1.85+ (edition 2024)

## Build

```
cargo build --release
```

The binary is at `target/release/filetransfer`.

## Run

```
./target/release/filetransfer
```

Default port is **4010**. Set the `PORT` environment variable to change it:

```
PORT=8080 ./target/release/filetransfer
```

Then open `http://localhost:4010` (or your chosen port) in a browser.

## Configuration

| Variable | Default | Description              |
|----------|---------|--------------------------|
| `PORT`   | `4010`  | TCP port to listen on    |
| `RUST_LOG`| `filetransfer=info` | Log level (uses `tracing` env filter syntax) |

## Project structure

```
src/
  main.rs          -- entry point, server setup, cleanup task
  state.rs         -- shared state, transfer lifecycle types, channel config
  routes.rs        -- HTTP and WebSocket upgrade handlers
  ws.rs            -- WebSocket logic for sender and receiver relay
  static_assets.rs -- embedded HTML (sender + receiver pages)
static/
  sender.html      -- sender UI, encryption, pipelined upload
  receiver.html    -- receiver UI, decryption, file assembly
```

## Security model

- Encryption key never leaves the browser and is never sent to the server. It exists only in the URL fragment.
- Each chunk is encrypted with AES-256-GCM using a unique 12-byte random IV prepended to the ciphertext.
- The server relays opaque binary blobs. It has no access to filenames in transit (only the initial metadata for the download page), and cannot decrypt the file contents.
- Transfers are ephemeral. No data is persisted to disk. Completed transfer records are cleaned up automatically.

## Performance notes

- Sender uses a producer-consumer pipeline: encryption runs ahead of network sends, overlapping CPU and I/O.
- 1 MB chunk size reduces per-chunk overhead (IV generation, promise dispatch, WebSocket frame headers).
- Receiver decrypts up to 6 chunks concurrently while preserving chunk ordering.
- Background-tab safe: uses `MessageChannel` instead of `setTimeout` to avoid browser timer throttling.
- Transfer speed is bounded by the slowest link in the chain: sender upload, server throughput, or receiver download.
