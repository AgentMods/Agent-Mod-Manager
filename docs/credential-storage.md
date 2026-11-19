# Credential Storage

Credentials no longer use any operating-system keychain API. Rust uses the
official Tauri Store plugin, storing only versioned AES-256-GCM envelopes with
a fresh 96-bit OS-random nonce on every write. Each credential name is bound
as authenticated additional data; moving ciphertext to another name fails.

## Runtime Files

Within the application's data directory:

- `private/credentials.json`: Tauri Store containing encrypted credentials.
- `private/.env`: `AMM_STORE_ENCRYPTION_KEY`, a 32-byte random key encoded as
  exactly 64 hexadecimal characters. Automatically generated on first use.

The `.env` file is parsed by Rust at runtime without exporting its contents
into the process environment. It is not a frontend `VITE_` variable and is not
compiled or bundled into the application. To use an existing separately managed
file, launch the app with `AMM_STORE_ENV_FILE` set to its absolute `.env` or
`.env.*` path. That
file must already exist; the app never generates or overwrites a custom file.
Project `.env` files are not discovered implicitly from a working directory.

On macOS/Linux the generated directory is mode `700` and files are mode `600`.
Unsafe files, symlinks, hardlinks and broad Unix file permissions are rejected.
On Windows protect the application data and custom .env file with a per-user
ACL. The app's FileEngine and read tools cannot access the private directory
or .env files. No Store permissions are granted to JavaScript: encryption and
decryption stay in Rust and the frontend only submits credentials for saving.

SQLite still stores public settings, secret reference names, game data and
conversation/transaction records. API keys, provider keys and local/external
MCP credentials use the encrypted Store. There is no plaintext fallback.

## Migration And Recovery

Old keychain references are ignored, not automatically migrated. Enter each
API key again in Settings; the existing keychain item is untouched, and the app
does not request access to it. Restart the rebuilt app to use the new backend.

Keep both runtime files backed up securely. If ciphertext exists and the .env
file is missing, the app refuses to generate a replacement key. Wrong keys,
corrupt JSON and invalid authentication tags fail without overwriting the
original credentials. Editing the key alone is not key rotation: existing
records must be decrypted with the old key and re-encrypted deliberately.

The master key is itself plaintext in .env, as requested. Anyone who can read
both files can decrypt the credentials. This is not equivalent to OS-backed
key isolation and does not protect against a compromised user account.
