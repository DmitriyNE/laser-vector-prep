# Deploying Laser Vector Prep

The app is a static site: `index.html`, `coi-serviceworker.js` and an `assets/` folder of
hashed JavaScript, CSS and WebAssembly files. There is no server code, no runtime CDN and
no telemetry; files a user opens never leave their browser. Any static host can serve it.

The public build is at <https://dmitriyne.github.io/laser-vector-prep/>, deployed by CI
from `main` (see [`development.md`](development.md#continuous-integration-and-deployment)).

## Getting the files

| How | Result |
|---|---|
| `nix develop -c just build` | `web/dist/` |
| `nix develop -c just package` | `dist/laser-vector-prep-<version>+<commit>.tar.gz` and its `.sha256`: the site plus `LICENSE` and this page as `DEPLOY.md`. The same commit always gives the same bytes. |
| `nix build` | `result/`: the site built hermetically from the flake, without a dev shell |

Copy the contents of the folder (or the unpacked tarball's top folder) to the web root or to
any sub-path. All URLs are relative, so `https://example.org/tools/laser/` works as well as
a domain of its own.

## Requirements

**HTTPS.** Browsers allow `SharedArrayBuffer` and service workers only in a secure context:
HTTPS, or `http://localhost` for testing.

**Cross-origin isolation.** The geometry engine uses WebAssembly threads, which need the
page to be cross-origin isolated. Either way works:

- *The server sends the headers* (preferred; the first visit loads directly):

  ```text
  Cross-Origin-Opener-Policy: same-origin
  Cross-Origin-Embedder-Policy: require-corp
  ```

- *The server cannot send headers* (GitHub Pages, some object stores): nothing to do. The
  bundled `coi-serviceworker.js` adds them; on the very first visit the page registers it,
  waits until it has cached the app, and reloads once.

Header examples:

```nginx
# nginx
location /laser/ {
    add_header Cross-Origin-Opener-Policy same-origin always;
    add_header Cross-Origin-Embedder-Policy require-corp always;
}
```

```apache
# Apache (.htaccess, mod_headers)
Header always set Cross-Origin-Opener-Policy "same-origin"
Header always set Cross-Origin-Embedder-Policy "require-corp"
```

```text
# Netlify / Cloudflare Pages: a file named _headers next to index.html
/*
  Cross-Origin-Opener-Policy: same-origin
  Cross-Origin-Embedder-Policy: require-corp
```

```caddyfile
# Caddy
header Cross-Origin-Opener-Policy same-origin
header Cross-Origin-Embedder-Policy require-corp
```

**Content type.** Serve `.wasm` as `application/wasm` (most servers do). With another type
the app still starts, but the browser compiles the module only after downloading it.

## Caching and updates

- `assets/*` file names contain a content hash: cache them for as long as you like.
- Serve `index.html` and `coi-serviceworker.js` so browsers revalidate them (for example
  `Cache-Control: no-cache`). After a deployment the service worker notices the new asset
  list on the next visit, caches the new version and discards the old one.

## Offline use

After the first visit the service worker has cached the whole app: it opens and works
without a network connection, including import, evaluation and export. Saved profiles live
in the browser's IndexedDB on that device.

## Browsers

Current Chrome, Edge and Firefox. Safari is not supported. The app needs module workers,
WebAssembly threads and a service worker or the isolation headers.

## Checking a deployment

Open the page and the browser's developer console:

- `crossOriginIsolated` is `true`;
- the profiles bar shows the core version and a thread count (`core 0.1.0 · 10 threads`);
- importing one of the repository's `fixtures/` files and exporting a PDF works, and keeps
  working after going offline and reloading.
