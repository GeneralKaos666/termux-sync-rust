# HTTP + GitHub Releases storage for Termux aarch64

Scope: how to replace `GitHubStorage` (`termux-sync.py` lines 1134-1573, plus
`_http_upload_file` at 312) and `_PartWriter` (1580-1667) in Rust, on Termux aarch64,
without OpenSSL, streaming 200 MB parts from disk.

Companion decisions: ADR-005 (Termux aarch64, rustls, never OpenSSL).

## 1. Recommendation

Use `ureq` 3 with its default `rustls` backend and the default `webpki-roots` trust
store. Do not use `reqwest` blocking on this target with default features.

Evidence measured on this device (Termux aarch64, `rustc 1.100.0`, `cargo 1.100.0`,
2026-11-24):

| Probe | Result |
|---|---|
| `ureq = "3"`, `default-features = false`, `features = ["rustls"]`, dev build | `Finished ... in 3m 10s`; compiled `ring v0.17.14`, `rustls v0.23.46`, `webpki-roots v1.0.12`, `ureq v3.5.2` |
| Same probe, `GET https://api.github.com/` at runtime | `Ok(200)` |
| `reqwest = "0.13"`, `features = ["blocking"]`, dev build (default features) | `Finished ... in 11m 56s` |
| Same reqwest client, `Client::builder().build()` then `GET https://api.github.com/` | panic: `Expect rustls-platform-verifier to be initialized` at `rustls-platform-verifier-0.7.1/src/android.rs:90`, then `event loop thread panicked` |

Why reqwest fails here: its 0.13 default feature chain is
`default = ["default-tls", ...]` → `default-tls = ["rustls"]` →
`rustls = ["__rustls-aws-lc-rs", "dep:rustls-platform-verifier", "__rustls"]`
(`reqwest-0.13.5/Cargo.toml`, `[features]`). On Android
`rustls-platform-verifier` "needs to call into the JVM. A small Kotlin component must
be included in your app's build", and `init_with_env`/`init_with_refs`/`init_with_runtime`
must run "before any networking has a chance to run"
(<https://docs.rs/rustls-platform-verifier/latest/rustls_platform_verifier/>). A Termux
CLI has no JVM, no `Activity`, and no `Context`, so it can never satisfy that contract.
`reqwest` only falls back to a WebPKI verifier when you supply custom roots
(`reqwest-0.13.5/src/async_impl/client.rs:807`), and those roots must be loaded by hand
via `tls_certs_only` / `add_root_certificate`.

`ureq` reaches the same place with less work: rustls with the `ring` provider, static
Mozilla roots by default, no tokio, no C toolchain step beyond `ring` (which built here),
no OpenSSL.

Trade-off accepted: `ureq` is an HTTP/1.1 client. Its documentation only describes
HTTP/1.1 framing and its request body chapter is titled "HTTP/1.1 has two ways"
(<https://docs.rs/ureq/latest/ureq/>). GitHub serves HTTP/1.1 fine, so this is not a
blocker. `reqwest` would give HTTP/2, at the cost of tokio plus the TLS problems above.

## 2. Crate set

```toml
ureq = { version = "3.5", default-features = false, features = ["rustls", "json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sha2 = "0.10"                                   # whole-stream digest, same as hashlib
fs4 = { version = "1", features = ["sync"] }    # available_space / free_space
```

Notes:

- `rustls` is a default feature of `ureq`, but naming it keeps intent explicit and
  survives a change of defaults ("The default enabled features are: **rustls** and
  **gzip**", <https://docs.rs/ureq/latest/ureq/>).
- Do **not** enable `ureq`'s `platform-verifier` feature on this target, for the reason
  in section 1. `RootCerts::PlatformVerifier` "requires the feature
  **platform-verifier**" and uses `rustls-platform-verifier`
  (<https://docs.rs/ureq/latest/ureq/tls/enum.RootCerts.html>).
- `webpki-roots` is the default: "By default, ureq uses Mozilla's root certificates via
  the webpki-roots crate. This is a static bundle of root certificates that do not update
  automatically" (<https://docs.rs/ureq/latest/ureq/>). Bumping `ureq` refreshes them.
  That is acceptable for one fixed host (`api.github.com`), which is exactly the case
  where the ureq docs call webpki-roots "likely sufficient".
- Do not add `native-tls`; it is never selected by default and would pull OpenSSL on
  this platform ("`native-tls` is never picked up as a default or used by the crate level
  convenience calls", same page).

## 3. Client configuration

One `Agent` for the whole process. It pools connections and is cheap to clone
(<https://docs.rs/ureq/latest/ureq/>).

```rust
use std::time::Duration;
use ureq::Agent;

fn agent() -> Agent {
    Agent::config_builder()
        .https_only(true)
        // Critical: keep 4xx/5xx responses so their JSON body can be read.
        .http_status_as_error(false)
        .max_redirects(10)              // default 10; error when exceeded
        .max_response_header_size(64 * 1024) // default 64 KiB
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        // Python's urllib timeout is per socket operation, not per transfer.
        .timeout_per_read(Some(Duration::from_secs(120)))
        .timeout_per_write(Some(Duration::from_secs(120)))
        .user_agent(concat!("termux-sync/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}
```

Basis for each setting:

- `http_status_as_error(false)`: "Whether to treat 4xx and 5xx HTTP status codes as
  `Err(Error::StatusCode))`. Defaults to `true`." With the default, the body is gone:
  `Error::StatusCode(u16)` carries only the number
  (<https://docs.rs/ureq/latest/ureq/config/struct.ConfigBuilder.html>,
  <https://docs.rs/ureq/latest/ureq/enum.Error.html>). Reading GitHub's `message`,
  `errors[].code` and `already_exists` requires the body, so this flag is mandatory.
- `max_redirects(10)`, error on overflow: documented defaults on `ConfigBuilder`.
  Set `max_redirects_will_error(false)` only if you want to inspect the 3xx yourself.
- `timeout_*`: "The various timeouts" table and per-method docs
  (<https://docs.rs/ureq/latest/ureq/config/struct.ConfigBuilder.html>). Important:
  `timeout_recv_body` and `timeout_send_body` are totals; do **not** set them for
  200 MB transfers or a slow link aborts a healthy upload. "Global, per-call, and
  response body budgets continue after a response is returned"; `None` disables a limit.
- `Authorization` on redirect: `redirect_auth_headers` "Defaults to `None`" (meaning
  `Never`) — "the authorization header is never attached to a redirected call".
  This is the behavior we want: `api.github.com` answers an asset download with a 302 to
  `objects.githubusercontent.com`, and the token must not travel there. Python's
  `urllib` forwards request headers across the redirect; the Rust port will not. Keep the
  default and note the intentional difference.
- Proxy: reading env is automatic for default agents: "Proxy variables are checked in
  order: `ALL_PROXY`, `HTTPS_PROXY`, then `HTTP_PROXY` (with lowercase variants).
  `NO_PROXY` specifies hosts that bypass the proxy"
  (<https://docs.rs/ureq/latest/ureq/>).

## 4. GitHub endpoints and status handling

Base URL `https://api.github.com`, asset upload host `https://uploads.github.com`.
All numbers below are from the GitHub REST reference.

| Operation | Request | Expected | Notes |
|---|---|---|---|
| Repo preflight | `GET /repos/{repo}` | 200, 404 | 404 also means "private, token cannot see it" |
| Empty-repo probe | `GET /repos/{repo}/commits?per_page=1` | 200, **409**, 422 | 409 Conflict is documented for this endpoint (<https://docs.github.com/en/rest/commits/commits>) |
| List releases | `GET /repos/{repo}/releases?per_page=100` | 200, 404 | paginated; `per_page` max 100 |
| Release by tag | `GET /repos/{repo}/releases/tags/{tag}` | 200, 404 | 404 is the normal "not yet" answer |
| Create release | `POST /repos/{repo}/releases` | **201**, 404, 422 | 201, not 200 |
| Delete release | `DELETE /repos/{repo}/releases/{id}` | **204**, 404 | no body |
| List assets | `GET /repos/{repo}/releases/{id}/assets` | 200 | paginated, default 30 |
| Upload asset | `POST {upload_url}?name=...` | **201**, 422, 502 | 422 = same filename exists |
| Get asset | `GET /repos/{repo}/releases/assets/{id}` | **200 or 302**, 404 | with `Accept: application/octet-stream` |
| Delete asset | `DELETE /repos/{repo}/releases/assets/{id}` | **204**, 404 | |
| Delete tag ref | `DELETE /repos/{repo}/git/refs/tags/{tag}` | 204, 409, 422 | 422 if absent or default branch |

The asset endpoint is explicit about both shapes: "set the Accept header ... to
`application/octet-stream`. The API will either redirect the client to the location, or
stream it directly if possible. API clients should handle both a 200 or 302 response."
(<https://docs.github.com/en/rest/releases/assets>). With redirects enabled, `ureq`
handles this; treat any final 200 as success and keep a guard that accepts 302 if
redirects are ever disabled.

Upload URL is hypermedia: "This key is a hypermedia resource"; "All URLs are expected to
be proper RFC 6570 URI templates" (<https://docs.github.com/en/rest/releases/releases>,
<https://docs.github.com/en/rest/using-the-rest-api/getting-started-with-the-rest-api>).
So read `upload_url` from the create/get-release response, strip a trailing
`{?name,label}` template, then add `?name=` with `RequestBuilder::query`
(`ureq-3.5.2/src/request.rs:170`). Hard-coding
`https://uploads.github.com/repos/{repo}/releases/{id}/assets` is equivalent today
(that is what Python does) and is a safe fallback.

Headers to send on API calls: `Authorization: Bearer <token>` (or `token`, both are
accepted: "in most cases you can use `Authorization: Bearer` or `Authorization: token`"),
`Accept: application/vnd.github+json`. Keep Python's
`Accept: application/vnd.github.v3+json` if you want byte-identical requests; both work.
A valid `User-Agent` is mandatory: "Requests with no `User-Agent` header will be
rejected. If you provide an invalid `User-Agent` header, you will receive a `403
Forbidden`" (<https://docs.github.com/en/rest/using-the-rest-api/getting-started-with-the-rest-api>).
`ureq`'s default `ureq/<version>` would pass, but set the app name.

`X-GitHub-Api-Version` is not sent by Python. Sending a pinned value (for example
`2022-11-28`) freezes response shape; that is a behavior change and needs an ADR before
it is added.

### Pagination

Use the `link` header, not `page=` arithmetic: "You should not try to ... manually
construct pagination queries. Instead, you should use the link headers"
(<https://docs.github.com/en/rest/using-the-rest-api/using-pagination-in-the-rest-api>).
Parse `<url>; rel="next"`, stop when absent. Python's `_list_releases` reads only the
first page of 100; with one release per backup that is 100 backups. Either keep the
single-page behavior for parity or paginate and record the difference.

## 5. Streaming a 200 MB part without buffering

GitHub requires a declared length: "Most libraries will set the required Content-Length
header automatically"
(<https://docs.github.com/en/rest/releases/assets>). Do not rely on chunked transfer.

`ureq` picks chunked for reader bodies:

- `SendBody::from_reader(&mut reader)` / `from_owned_reader` are listed under
  "Transfer-Encoding: chunked" (<https://docs.rs/ureq/latest/ureq/>).
- Setting `Content-Length` yourself suppresses that: the header map is checked first,
  and when it already declares a body mode, `ureq` does not add one
  (`ureq-3.5.2/src/run.rs:283-340`, `ureq-3.5.2/src/util.rs:368`). The crate docs say the
  same: "If you set your own Content-Length or Transfer-Encoding header before sending the
  body, ureq will respect that header".
- Over-sending is rejected: `consume_direct_write` returns
  `BodyLargerThanContentLength` when a write exceeds the declared length
  (`ureq-proto-0.7.1/src/client/sendbody.rs:51`), surfaced as
  `Error::BodyExceedsLimit(u64)`.
- Under-sending does not error on its own: the body loop keeps reading until the declared
  length is satisfied. Bound it with `Read::take(len)` **and** a `timeout_send_body`.

Actionable upload:

```rust
fn upload_asset(
    agent: &ureq::Agent,
    upload_url: &str,
    asset_name: &str,
    path: &std::path::Path,
    auth: &str,
    mut progress: impl FnMut(u64),
) -> Result<serde_json::Value, ApiError> {
    let file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let mut body = std::io::BufReader::with_capacity(128 * 1024, file).take(len);
    let counted = CountingReader::new(&mut body, len, &mut progress);

    let resp = agent
        .post(upload_url)
        .query("name", asset_name)
        .header("Authorization", auth)
        .header("Accept", "application/vnd.github+json")
        .header("Content-Type", "application/octet-stream")
        .header("Content-Length", &len.to_string())
        .config()
        .timeout_send_body(Some(std::time::Duration::from_secs(6 * 3600)))
        .build()
        .send(ureq::SendBody::from_reader(counted))
        .map_err(ApiError::Transport)?;

    match resp.status().as_u16() {
        201 => Ok(read_json(resp)?),
        422 => Err(ApiError::from_response(resp)),   // maybe already_exists
        code => Err(ApiError::from_status(code, resp)),
    }
}
```

Points:

- `SendBody` is `!Send` (<https://docs.rs/ureq/latest/ureq/struct.SendBody.html>), so
  upload on the thread that owns the file. That is fine: the CLI is synchronous.
- Per-request overrides (the long `timeout_send_body` above) go through
  `RequestBuilder::config()`, which returns a `ConfigBuilder<RequestScope<Any>>` whose
  `build()` gives the same request back (`ureq-3.5.2/src/request.rs:368`).
- `Content-Type: application/octet-stream` is the documented media type for raw asset
  data ("GitHub expects the asset data in its raw binary form")
  (<https://docs.github.com/en/rest/releases/assets>).
- No resumable upload exists in the API: one POST per asset, one body. A retry restarts
  the whole 200 MB part.
- 502 has a side effect worth handling: "When an upstream failure occurs, you will
  receive a 502 Bad Gateway status. This may leave an empty asset with a state of
  starter. It can be safely deleted." So a retry after 502 must first list assets and
  delete the same-named starter asset, otherwise the retry gets 422.

## 6. Streaming a download

```rust
let resp = agent
    .get(&asset_api_url)                  // /releases/assets/{id}
    .header("Authorization", auth)
    .header("Accept", "application/octet-stream")
    .call()?;
let mut reader = resp.into_body().into_reader();   // owned, 'static, unlimited
std::io::copy(&mut CountingReader::new(reader, total, &mut progress), &mut out)?;
```

- `as_reader()`/`into_reader()` are "not limited by default"; only `read_to_vec`,
  `read_to_string` and `read_json` apply the 10 MB default limit
  (<https://docs.rs/ureq/latest/ureq/struct.Body.html>). Use the reader for archives.
- The reader "will error if `Content-Length` is set, but the connection is closed before
  all bytes are received" — that gives truncation detection for free. Additionally
  compare bytes written with the asset `size`.
- Do not set `timeout_recv_body` here; a 200 MB part on mobile data would trip it. Use
  `timeout_per_read`.
- `serde_json` on the manifest is small; the default 10 MB limit is enough, but make the
  limit explicit so the error is `BodyExceedsLimit` instead of a surprise.

## 7. Part writer state machine (parity with `_PartWriter`)

Rules to preserve exactly (ADR-004):

1. Part name is `{archive_name}.part{idx:04d}` where `archive_name` is the tar name,
   e.g. `home.tar.gz.part0000`. Python builds it from `self.base` in
   `_PartWriter.write` (line 1621).
2. Small archive: if the stream finishes in part 0, rename `...part0000` to the plain
   archive name and upload that, so no `.part` suffix exists (lines 1638-1641).
3. Each full part is uploaded and then **deleted from disk** before the next is opened
   (lines 1643-1650). Peak disk use is one part plus the part being written.
4. The whole stream is hashed with SHA-256 as it is written, and `total` is the byte
   count of the whole archive; both land in the manifest (lines 1626-1627, 1819-1824).
5. On any upload failure, raise `StreamAbort`, which the backup path turns into
   `abort_stream` → `delete_backup(backup_name)`, so a half release never lists as a
   backup (lines 1644-1647, 1258-1263).

Rust shape:

```rust
struct PartWriter<'a> {
    storage: &'a mut GitHubStorage,
    release_id: i64,
    work_dir: PathBuf,
    base: String,          // e.g. "home.tar.gz"
    idx: u32,
    total: u64,
    hasher: Sha256,
    file: Option<BufWriter<File>>,
    path: Option<PathBuf>,
    size: u64,
    limit: u64,            // 200 * 1024 * 1024
}
```

Implement `std::io::Write`; on `size >= limit`, flush, close, upload, `remove_file`,
`idx += 1`. `finish()` uploads the tail. `abort()` closes and removes the partial file.
No async, no channels: one writer thread keeps delete-after-upload ordering simple.

Do not buffer a part in memory to upload it. Python's non-event path does
`fobj.read()` (line 1346) and that is the path to avoid; `_http_upload_file` (line 312)
already streams in 256 KiB slices, and `ureq` + `Content-Length` reproduces it.

## 8. Free space check

Python: `need = CHUNK_SIZE * 1.2`, `shutil.disk_usage(work_dir).free`, and `ENOSPC` is
rethrown immediately instead of retried (lines 1241-1250, 1374-1375, 1514-1515).

Rust: `fs4::free_space(path)?` returns "the number of free bytes in the file system
containing the provided path"; `fs4::available_space` returns the same for non-privileged
users (<https://docs.rs/fs4/latest/fs4/>). `shutil.disk_usage().free` is the free-block
count, so `free_space` is the exact equivalent. Keep the 1.2 factor and the same
`UserError` text.

For `ENOSPC`, map `std::io::Error` via `raw_os_error() == Some(libc::ENOSPC)` and mark it
non-retryable, same as Python.

## 9. Error decoding, retries, rate limits

GitHub error bodies are JSON with `message` and, for validation failures, `errors`:
"The response body will include an `errors` property, which includes a `code` property";
documented codes include `missing`, `missing_field`, `invalid`, `already_exists`,
`unprocessable`, `custom`
(<https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api>).
Python builds its message from `message` plus `errors[].message` (lines 1181-1231).

Implement a tolerant parser:

```rust
#[derive(serde::Deserialize)]
struct GhError { message: Option<String>, errors: Option<Vec<GhErrorDetail>> }
#[derive(serde::Deserialize)]
struct GhErrorDetail { message: Option<String>, code: Option<String> }
```

- Missing/extra fields must not fail the decode; GitHub adds fields over time.
- Non-JSON bodies (proxy error pages, 502 HTML) fall back to the first 300 characters,
  matching Python's fallback.
- Keep Python's mapping verbatim: 409/422 + "empty" → empty-repository `UserError`; 401 →
  invalid token; 403 + "rate limit" → rate limit; 403 → denied; 404 → not found (with the
  private-repo hint); otherwise `GitHub API error {code}: {detail}`.
- 404 semantics: "GitHub uses a 404 Not Found response instead of a 403 Forbidden
  response to avoid confirming the existence of private repositories" (same page). Never
  treat 404 as "retry later".

Retry matrix:

| Class | Retry? |
|---|---|
| transport: timeout, connection reset, DNS, broken pipe, TLS handshake | yes |
| 429 | yes, wait |
| 5xx, including 502 on upload | yes, after cleaning a starter asset |
| 403 with `x-ratelimit-remaining: 0` or `retry-after` or "rate limit" text | yes, wait |
| 401, 403 (permission), 404, 409, 422 (except `already_exists` upload) | no |
| `ENOSPC` | no |
| `already_exists` on upload | skip that asset, no retry (Python lines 1362-1365) |

Backoff: GitHub's rules are explicit. "If the `retry-after` response header is present,
you should not retry ... until after that many seconds. If the `x-ratelimit-remaining`
header is `0`, ... until the time specified by the `x-ratelimit-reset` ... Otherwise, wait
for at least one minute"
(<https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api>).
Python uses `15 * (attempt + 1)` and 8 attempts. Suggested port: keep
`15 * (attempt + 1)` as the floor, take `max(floor, retry-after, reset-now)`, 8 attempts,
and cap the wait so an absurd `reset` cannot hang the CLI forever.
"Continuing to make requests while you are rate limited may result in the banning of your
integration" — do not retry-loop on 403 without honoring the wait.

Rate-limit facts that shape the design: authenticated limit 5,000 requests/hour;
secondary limits: "No more than 100 concurrent requests", "no more than 900 points per
minute", mutative requests cost 5 points, and "wait at least one second between each
request" for POST/PATCH/PUT/DELETE. The port is serial already, but `delete_old` deletes
release + tag per backup, so add the 1 s pause between mutative calls or expect 403
"secondary rate limit" on large cleanups.

Headers to read: `x-ratelimit-limit`, `-remaining`, `-used`, `-reset`,
`-resource`, `retry-after`. Values fluctuate between regions; do not make correctness
depend on exact remaining counts ("rely on the response headers" and "avoid logic that
depends on an exact remaining count").

## 10. Termux specifics

- **TLS roots.** Use `webpki-roots` (ureq default). Do not use the Android system store
  from a CLI: `rustls-platform-verifier` needs the JVM/Kotlin component and explicit
  initialization on Android. On Linux the same crate uses the system CA bundle via
  `rustls-native-certs`; Termux runs against bionic, not glibc, so that path is not
  relevant.
- **Android CA files exist but are not reachable.** On this device
  `/system/etc/security/cacerts/` is populated and Termux ships `ca-certificates` at
  `$PREFIX/etc/tls/cert.pem`, but neither is consumed by webpki-roots. That is fine for a
  fixed host; document that a corporate MITM proxy will break `api.github.com` unless
  extra roots are supplied via `RootCerts::Specific` /
  `RootCerts::new_with_certs`
  (<https://docs.rs/ureq/latest/ureq/tls/enum.RootCerts.html>).
- **DNS.** There is no `/etc/resolv.conf` on this device (checked: absent), and that does
  not matter. `ureq`'s default resolver calls `to_socket_addrs()`
  (`ureq-3.5.2/src/unversioned/resolver.rs:118`), i.e. libc `getaddrinfo`, which Android
  routes through netd. The probe reached `api.github.com` with no resolver file present.
  Do not add `hickory-dns` (a reqwest-only feature) unless Android DNS is proven broken.
- **No OpenSSL.** rustls + `ring` only. `ring` built from source with the Termux clang
  (`clang 21.1.8`, `aarch64-unknown-linux-android24`) in 3m10s including rustls and ureq.
  Nothing in the dependency tree links `libssl`.
- **Do not enable `system-proxy`-style platform integration.** `ureq` reads proxy env
  vars, which is the Termux-appropriate behavior.
- **Build cost.** `ureq` dev build 3m10s vs `reqwest` 11m56s on the same device. The
  reqwest build compiled `aws-lc-sys 0.47.0` from source (observed in the cargo registry
  and `target/debug/build`), which is the C/CMake toolchain path; `cmake` 4.5.1 and
  `perl` 5.44 are installed here, so it built — and the result still panics at runtime.

## 11. Semantic traps to encode in tests

1. **Error body lost by default.** With `http_status_as_error(true)` (the default),
   `ureq` returns `Error::StatusCode(u16)` and no body; the empty-repo hint, the
   `already_exists` skip and the rate-limit hint all stop working. Test that a 422 with
   `{"message":"Validation Failed","errors":[{"code":"already_exists"}]}` is treated as
   "skip asset", and that a 409 with `"empty"` produces the empty-repository `UserError`.
2. **201 vs 200.** Create release and upload asset return 201. A `== 200` check silently
   treats success as failure.
3. **204 is success with no body.** Delete release, delete asset, delete ref. Do not
   `read_json`; a premature decode turns success into an error.
4. **`already_exists` is not a hard failure, but the asset is not re-uploaded.** Python
   skips (line 1362). GitHub's rule is stricter: "you'll receive an error and must delete
   the old file before you can re-upload the new asset"
   (<https://docs.github.com/en/rest/releases/assets>). Keep the skip for parity; only
   build the delete-then-retry path for the 502-starter case.
5. **GitHub renames assets.** "GitHub renames asset filenames that have special
   characters, non-alphanumeric characters, and leading or trailing periods." Never
   assume the returned `name` equals the requested name; read names from responses and
   from `list assets` when reconciling. Current names (`termux_backup_*`,
   `home.tar.gz.part0000`) are safe.
6. **1000 assets per release, 2 GiB per asset.** "Up to 1000 release assets may be
   associated with a single release. Each file included in a release must be under 2 GiB.
   There is no limit on the total size"
   (<https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases>).
   A 200 MB chunk keeps each upload safely under 2 GiB; the 1000-asset cap is the real
   ceiling (200 GB per release before parts + manifest exhaust it).
7. **Empty repository.** `GET /commits` documents 409; release creation can answer 422.
   Detect by status and by the word "empty" in the body, as Python does. Do not retry
   either code.
8. **404 is ambiguous.** Missing repo, missing release, and "private and invisible to this
   token" all look the same. Keep Python's combined message and hint.
9. **Redirect on upload is not replayable.** For 301/302/303 ureq follows with a GET (curl
   behavior, `ureq-proto-0.7.1/src/client/redirect.rs:61`); for 307/308 with a body it
   returns `Error::RedirectFailed` ("ureq refuses to redirect the POST body",
   <https://docs.rs/ureq/latest/ureq/enum.Error.html>). Treat any 3xx from the upload URL
   as an error and re-POST to `Location` yourself if you choose to support it. A silent
   GET looks like a successful upload but stores nothing.
10. **Rate-limit retries must read headers.** A 403 secondary-limit response is not fixed
    by 15 s of sleep if `retry-after` is 300. Read the headers first. Also pause 1 s
    between mutative calls in `delete_old`.
11. **Partial upload/download is not resumable.** Retry restarts the part. Downloads must
    truncate the destination on each attempt (`File::create`, not `append`), which is what
    Python's `open(out, "wb")` does (line 1493).
12. **Truncated download detection.** `ureq`'s reader errors when `Content-Length` was
    promised and the peer closes early; also compare with the asset `size`.
13. **Part ordering and reassembly.** Reassemble by sorting names and matching
    `^(.+)\.part(\d{4})$`, then delete the parts, exactly as lines 1527-1545. Four-digit
    indices are load-bearing.
14. **No `.part` suffix for single-part archives.** If a backup producer ever changes the
    small-archive rename, restore breaks for archives < 200 MB.
15. **Manifest after parts.** Parts upload during archive creation; `manifest.json` uploads
    later through `upload()`. A crash between the two leaves a release without a manifest,
    and `list_backups` then falls back to `{name, date, label: ""}` (lines 1441-1459).
    Preserve that fallback.
16. **`delete_old` deletes the release and then the tag ref.** The ref delete can 422 if
    the tag is already gone; Python swallows it (lines 1556-1560). Keep swallowing, but do
    not swallow a 401/403 there silently or the retention loop will look successful while
    doing nothing.

## 12. Open decisions for the port

- Paginate `GET /releases` or keep the single page of 100? Pagination changes
  `list` output once a user exceeds 100 backups; ADR-004 argues for parity, ADR-003 only
  pins machine output semantics.
- Send `X-GitHub-Api-Version` and switch `Accept` to `application/vnd.github+json`, or
  keep Python's `v3+json` with no version header? Requests become future-proof but
  differ from the recorded cases.
- Delete-then-retry on `already_exists` (robust) vs skip (parity). Recommend skip now,
  revisit with a recorded case.
- Whether `abort_stream` should also delete the tag ref. Python calls `delete_backup`,
  which deletes both release and tag (lines 1258-1263, 1562-1570); a failed release
  creation leaves a tag if the ref was created by the release. Keep Python's behavior.

## References

- ureq crate docs (features, TLS, roots, bodies, proxies):
  <https://docs.rs/ureq/latest/ureq/>
- `ureq` `ConfigBuilder` (status-as-error, redirects, auth-on-redirect, timeouts, buffers):
  <https://docs.rs/ureq/latest/ureq/config/struct.ConfigBuilder.html>
- `ureq` `Body` (10 MB default limit, unlimited readers, truncation error):
  <https://docs.rs/ureq/latest/ureq/struct.Body.html>
- `ureq` `SendBody`:
  <https://docs.rs/ureq/latest/ureq/struct.SendBody.html>
- `ureq` `Error` / `RootCerts`:
  <https://docs.rs/ureq/latest/ureq/enum.Error.html>,
  <https://docs.rs/ureq/latest/ureq/tls/enum.RootCerts.html>
- `ureq` source used for header precedence and redirect-method behavior:
  `ureq-3.5.2/src/run.rs`, `ureq-3.5.2/src/util.rs`,
  `ureq-proto-0.7.1/src/client/sendbody.rs`,
  `ureq-proto-0.7.1/src/client/redirect.rs`
- reqwest blocking API, `Body::new`/`Body::sized`, `ClientBuilder` defaults:
  <https://docs.rs/reqwest/latest/reqwest/blocking/>,
  <https://docs.rs/reqwest/latest/reqwest/blocking/struct.Body.html>,
  <https://docs.rs/reqwest/latest/reqwest/blocking/struct.ClientBuilder.html>
- reqwest feature graph: <https://docs.rs/crate/reqwest/latest/features> and
  `reqwest-0.13.5/Cargo.toml`
- rustls-platform-verifier (platform table, Android JVM requirement):
  <https://docs.rs/rustls-platform-verifier/latest/rustls_platform_verifier/>
- GitHub releases API: <https://docs.github.com/en/rest/releases/releases>
- GitHub release assets API: <https://docs.github.com/en/rest/releases/assets>
- GitHub commits API (409): <https://docs.github.com/en/rest/commits/commits>
- GitHub git refs API: <https://docs.github.com/en/rest/git/refs>
- Pagination: <https://docs.github.com/en/rest/using-the-rest-api/using-pagination-in-the-rest-api>
- Rate limits: <https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api>
- Best practices (redirects, serial requests, 1 s pause):
  <https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api>
- Error shapes and 404 semantics:
  <https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api>
- Getting started (hypermedia URI templates, User-Agent):
  <https://docs.github.com/en/rest/using-the-rest-api/getting-started-with-the-rest-api>
- Release limits: <https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases>
- fs4 space helpers: <https://docs.rs/fs4/latest/fs4/>
