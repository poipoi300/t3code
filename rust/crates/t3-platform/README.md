# t3-platform

Real hosts for `t3_client::platform::Platform`. One crate, one constructor per host family:

- `NativePlatform::open(NativeConfig)` for Windows, macOS, Linux, Android and iOS (tokio).
- `WebPlatform::open(WebConfig)` for browsers (`wasm32-unknown-unknown`).

Shells pass what only they know: the data directory, the secret-key store on mobile, client
presentation metadata, lifecycle events, and a `HostExtension` for UI-bound host actions.

## Native

Open the platform inside a tokio runtime built with time and I/O enabled
(`Builder::enable_all`), and poll the client's futures on that runtime: a current-thread
runtime driving them, or any executor while a multi-thread runtime runs. HTTP and socket
handshakes run as runtime tasks and are aborted when their future is dropped. Storage runs on
the blocking pool; a dropped storage call still commits or rolls back as a whole.

| Capability | Implementation |
| --- | --- |
| Storage | One SQLite file, `client-storage.sqlite`, in `NativeConfig::data_dir`. Both areas are rows of one table, so `transaction` is one SQLite `BEGIN IMMEDIATE` transaction: conditions are checked, ordinary then secure writes apply in order, and a failed condition writes nothing. WAL with `synchronous = FULL`. |
| Secure area | AES-256-GCM per value under a 32-byte key held by a `SecretKeyStore`; the encoded key tuple is the associated data, so a sealed value cannot be moved to another key. |
| HTTP | reqwest 0.12, see below. |
| WebSocket | tokio-tungstenite 0.28 with the same rustls config. 100 MiB message cap (the server's), subprotocols offered and checked like a browser, cancel-safe `receive`, `close` bounded by `Timeouts::close`. Handshake errors never echo the ticket URL. |
| Clocks | `SystemTime` for epoch, tokio's `Instant` for monotonic time, so tests on a paused clock see consistent time. |
| Randomness | `ring::rand::SystemRandom` (the OS generator). |
| Events | `StorageChanged` for every key a committed write or transaction touched; everything else comes from the shell through `NativePlatform::event_sender()`. |

### HTTP client choice

reqwest with default features off and only `rustls-tls-webpki-roots-no-provider` (plus
`system-proxy` on desktop), configured with the same `rustls` `ClientConfig` as the WebSocket:
ring provider, and on Windows, macOS and Linux the OS trust store through
`rustls-platform-verifier` (Mozilla roots from `webpki-roots` on Android and iOS).
`NativeConfig::extra_roots` adds a deployment's own CA without touching the OS store. It was already in the workspace lockfile through three crates, so it adds no new
transitive dependencies, and it runs on the tokio runtime the socket needs anyway. Redirects are
off (`Policy::none()`, the 3xx is returned unfollowed), there is no cookie store, no HTTP/2,
HTTP follows the system proxy on desktop as Chromium does: on Windows and macOS the OS settings
only, never environment variables, with the OS bypass list and its "simple hostnames go direct"
rule (`<local>`); on Linux the environment. Loopback and link-local addresses, in every spelling,
always go direct. Mobile uses no proxy, connect and whole-request timeouts come from `Timeouts`, and the body is
read chunk by chunk so a response over `max_response_bytes` fails with `TooLarge` without
being buffered, with or without a `Content-Length`. `ureq` is smaller but blocking, and a raw
hyper client would mean owning connection and TLS setup code.

`Credentials` has no effect on native: there is no ambient cookie jar.

### Known differences from the TypeScript clients

- **Android TLS trusts only the Mozilla roots** (not approved yet). A server behind a private or
  enterprise CA that the user installed on the device does not connect from Android, where the
  React Native app trusts the system and user stores. Needed: the verifier's Android path, which
  ships a Kotlin component in the APK (`rustls-platform-verifier-android`) and needs JNI
  initialisation with the app's `Context` (the Java VM is already reachable, U13). About a day,
  plus a device check.
- **WebSockets ignore the proxy** (not approved yet). On a network where only a proxy reaches the
  internet, HTTP works on desktop but the live connection does not, so environments never get
  past connecting. `tokio-tungstenite` has no proxy support: the socket needs a hand-written
  HTTP `CONNECT` through the system proxy (with its auth), then TLS over the tunnel. About a day
  and a half with tests against a local proxy.

### Secret storage per OS

| OS | Store | Notes |
| --- | --- | --- |
| Windows | Credential Manager (`keyring`, `windows-native`) | Electron's `safeStorage` uses DPAPI; both are bound to the user account. |
| macOS | Keychain (`keyring`, `apple-native`) | Same facility `safeStorage` uses. Not built on this machine. |
| Linux | Secret Service over D-Bus (`keyring`, `async-secret-service`, pure-Rust zbus) | Like `safeStorage`, it refuses to open when no secret service is running instead of falling back to `basic_text`. Not built on this machine. |
| Android | `AndroidKeystoreStore` (`NativeConfig::mobile`): the storage key wrapped by an AES-GCM key in the Android Keystore, the wrapped form fsynced beside the data | The Java VM comes from `set_android_vm` or, on API 31+, the NDK's `JNI_GetCreatedJavaVMs`. Never run on a device. |
| iOS | `KeychainStore` (`NativeConfig::mobile`): a generic password item | Defaults only; see the shell requirement below. Not built on this machine. |

Only the storage key lives in the OS store (service `T3 Code client storage`, account
`storage-<random id>` recorded in the database), not every secret, so values have no size limit
and stay inside the cross-area transaction. If the OS store is unavailable, `open` fails with
`OpenError::SecretStore`; nothing is written in plaintext. If the key is gone (the user cleared
the keychain) or unreadable (a damaged Android wrapped key, a Keystore key that no longer opens
it, a wrong length), a new key replaces it under a **new** id and the secure rows are kept,
unreadable, under the old one (`retained_secure_records` in `NativePlatform::open_report()`):
a key that is missing for now (a roaming credential, a collection still unlocking, a Keystore
error) is no proof it is gone. When the old key reads again on a later launch, its rows are
re-sealed under the current key (`recovered_secure_records`), a newer row of the same key
winning, and the old entry is deleted. One generation is kept; rows of an older one, or of a key
an older build overwrote, are discarded (`discarded_secure_records`).

Every read of the OS store runs with no database lock held, so a keychain or Secret Service
prompt never keeps another process waiting. Writing a new key, the rare path (first run, a key
lost), takes the database's write lock and first re-reads the key and its recorded fingerprint,
so two instances starting together share one key. The key id is committed before a key is
stored, so a failed open reuses it, and switching the database to WAL retries while another
instance holds it. A database file SQLite reports as not a database or corrupt, at open or in
`PRAGMA quick_check`, is moved aside as `client-storage.sqlite.corrupt-<seconds>` and a new one
started (`recreated_database`); the next open deletes the copy, sealed rows and all, and then
the old key's OS entry when the copy's key id could be read. A copy that cannot be deleted is
logged and left. The report reaches the UI once, through `Platform::storage_report` and
`ClientRuntime::take_storage_notice`, which keeps it on disk until the UI has taken it. On Unix
the data directory is created `0700`; deleted secure rows are overwritten (`secure_delete`) and
the WAL is checkpointed after every secure delete, the open's own included.

**Shell requirement (iOS keychain accessibility).** `KeychainStore` sets only class, service and
account, so the item gets the system defaults: accessible when unlocked, migratable to a new
device through an encrypted backup, not synchronised, in the app's first access group. The TypeScript app stores voice grants
`WHEN_UNLOCKED_THIS_DEVICE_ONLY`. Before any secret that must not leave the device moves into the
secure area, or if an app extension or a background launch while locked must read storage, the
iOS shell has to choose the accessibility class and access group and pass them to the store (an
option `KeychainStore` does not take yet). `NativeConfig::desktop(dir)` uses the OS store;
`EphemeralSecretStore` keeps keys in memory for tests and throwaway tools.

## Web

| Capability | Implementation |
| --- | --- |
| Storage | IndexedDB database `t3code:client` (configurable), version 1, object stores `ordinary` and `secure`, out-of-line key = `StorageKey::encode_tuple()`, value `{ namespace, value: Uint8Array }`, index `namespace`. A conditional transaction spans both stores; conditions are compared inside the request callbacks and the writes are issued from the last one, so the transaction is never inactive between check and write. |
| Secure area | A separate store in the same origin-isolated database. Browsers offer nothing stronger; the TypeScript client keeps its bearer tokens in IndexedDB too. |
| HTTP | `fetch` with an `AbortController` (dropping the future aborts), `redirect: "error"` (a browser cannot return the 3xx), credentials mapped to `omit` / `same-origin` / `include`, body streamed through a reader against the cap. |
| WebSocket | `web_sys::WebSocket` with `arraybuffer` binary type; `receive` buffers in a channel and is cancel-safe; `close` waits for the close event for at most 5 s. |
| Clocks | `Date.now()`, `performance.now()`. |
| Randomness | `crypto.getRandomValues`, in 64 KiB chunks. |
| Timers | `setTimeout`, cleared on drop; sleeps longer than 2^31-1 ms are chained. |
| Events | `online`/`offline` → `Network`, `visibilitychange` → `Suspended`/`Foreground`, committed keys → `StorageChanged` here and, through a `BroadcastChannel`, in other tabs. `WebPlatform::network_status()` gives the starting status. |

### Relation to the TypeScript client's browser storage

The Rust client's `Storage` is one versioned `Record` per `StorageKey`. The TypeScript client
does not store records that way, so its stores are not reused for `Storage`:

- `t3code:connection-runtime` (v5): `catalog` holds one document with every connection
  profile, credential and remote DPoP token; `shell`, `thread`, `server-config`, `vcs-refs` and
  `query` hold bare cache values keyed by environment and thread ids. Writing Rust records there
  would break the TypeScript client sharing the origin.
- `t3code:project-favicons` / `images` and the `localStorage` keys
  (`t3code:composer-drafts:v1`, `t3code:last-editor`, `t3code:github-routing:*`, …) likewise.

They are exposed read-only through `LegacyStorage` with source `web` and namespaces
`indexeddb/<database>/<store>`, `localStorage` and `sessionStorage`; values come back as the
stored string's UTF-8, raw bytes, or JSON text. `t3_client::persistence::import_namespace`
turns them into records (`read_web_indexed_db`, `read_web_storage`). A missing legacy database
is reported as empty and is never created. Two pieces of identity are used in place, so a
profile keeps them without import:

- the DPoP proof key: `t3code:cloud-auth` / `keys` / `relay-dpop-proof-key`, same record shape
  (`{ privateKey: CryptoKey, publicJwk, thumbprint }`), private key non-extractable;
- the analytics device id: `localStorage["t3code:analytics-device-id"]`.

**Shell requirement (legacy credentials).** These stores stay read-only here, so a token the
TypeScript client saved (the `t3code:connection-runtime` catalog, `expo-secure-store`, the
Electron store) is still on disk after the environment is removed in the Rust client. Once a
shell stops shipping the TypeScript client beside it, it must clear those stores after a
successful import; until then removing an environment does not remove the legacy copy.

## Host actions

Implemented here:

| Action | Native desktop | Android / iOS | Web |
| --- | --- | --- | --- |
| `DpopThumbprint`, `DpopPublicJwk`, `DpopSign` | P-256 key (ring) in the secure area, ES256 r‖s signatures | same | WebCrypto key shared with the TypeScript client |
| `AnalyticsDeviceId` | persisted UUID, secure record `device-identity` / `agent-awareness` (the record the mobile importer writes) | same | `localStorage` id, as today |
| `RelayDeviceId` | `None`, as the desktop app sends today | the same persisted id, as the mobile app | `None`, as today |
| `ClientPresentation` | from `NativeConfig::presentation` | same | from `WebConfig::presentation` |
| `ClipboardRead` / `ClipboardWrite` | `arboard`, `text/plain` and `text/html` | extension | async clipboard API, `text/plain` and `text/html` |
| `OpenUrl` | `open`, what `external_links::safe_external_url` allows for the desktop (web links, editor and app-launch links; never `mailto`) | extension | `window.open(…, "noopener,noreferrer")`, web links only (`http`, `https`) |

Everything else goes to the shell's `HostExtension`, or fails with
`PlatformError::Unsupported("host action <Name> is not available on this host; …")` when there
is none. Who must provide them:

| Action | Provider |
| --- | --- |
| `NotificationPermission`, `Notify`, `CancelNotification` | Desktop shell (OS notifications), mobile shell (push and local notifications). The web could use the Notification API; not done here. |
| `Haptic` | Mobile shell. |
| `ClipboardRead`, `ClipboardWrite`, `OpenUrl` on Android and iOS | Mobile shell (`ClipboardManager`/`UIPasteboard`, intents/`UIApplication.open`). |
| `PickResources`, `ReadResource`, `WriteResource`, `Share` | Each shell's file picker and share sheet (UI). |
| `MicrophonePermission`, `StartRecording`, `StopRecording`, `PlayAudio`, `StopAudio` | An audio package per shell (voice input). |
| `CloudIdentity`, `CloudToken` | The cloud/Clerk session package. |
| `PrimaryBearer`, `Registrations` | Desktop shell: the bundled backend's bearer and saved environments. |
| `SshProvision`, `SshPrepare`, `SshDisconnect` | Desktop shell's SSH package. |
| `ClearEnvironment` | The shell that keeps per-environment state outside `Storage`. |

`LegacyStorage` on native delegates to `NativeConfig::legacy`; without one it is
`Unsupported`. Reading Electron's Chromium profile or the mobile app's `expo-secure-store`,
AsyncStorage and documents needs the shell.

## Smoke test

```sh
cd rust
T3_SMOKE_URL=http://127.0.0.1:3773 T3_SMOKE_PAIRING_TOKEN=... cargo run -p t3-platform --example smoke
# or T3_SMOKE_BEARER=... instead of the pairing token
```

It opens the native platform on a temp directory with in-memory keys and calls
`t3_client::connection::connect`. The shared client owns authorization, ticketing, RPC,
config and shell synchronization, and reconnects. The example prints the config, actual sync status
and thread titles, then closes and awaits the connection task. It never prints the credential.
See the [client entry point](../t3-client/README.md) for the UI lifetime contract.

## Required checks before a change to `t3-platform` or `t3-client`

Most of this crate is compiled per target (`cfg(target_os = …)`), so a desktop test run and the
wasm check do not compile the Android and iOS arms. Type-check one target of each as well:

```sh
cargo check -p t3-client -p t3-platform --target wasm32-unknown-unknown
cargo check -p t3-client -p t3-platform --target x86_64-linux-android
cargo check -p t3-client -p t3-platform --target aarch64-apple-ios-sim
```

Without the Android NDK or an Apple SDK, the build scripts of `ring` and `libsqlite3-sys` cannot
compile their C. `cargo check` never links, so a stand-in compiler that writes empty objects is
enough: set `CC_<target>`, `CXX_<target>` and `AR_<target>` (target with underscores) to it, and
for Apple targets `SDKROOT` and `IPHONEOS_DEPLOYMENT_TARGET` so the `cc` crate does not run
`xcrun`. With the NDK installed, its `clang` works too. When an item is gated to a list of
targets, every call site's condition must be a subset of the item's.

## Tests

Native tests run against loopback servers on ephemeral ports (a hand-written HTTP/1.1 server and
`t3-rpc`'s `AsyncServer` behind tokio-tungstenite) and temp directories. They do not touch the
OS clipboard, browser or credential store.

`cargo test -p t3-platform --features integration-server --test client_connection -- --nocapture`
also starts the real Rust server on an ephemeral loopback port, proves pairing, config and a
shell snapshot, a guarded RPC probe, reconnect and bearer restoration through the public client entry point,
and stops the captured child process. All server and client data is temporary.
The Rust server's shell subscription and mounted HTTP snapshot routes are still missing; this
test currently fails at the snapshot assertion after proving pairing, config and the RPC probe.
